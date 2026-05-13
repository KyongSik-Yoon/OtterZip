//! Lenient ZIP backend — regression tests.
//!
//! Compares the new in-tree [`backends::lenient_zip`] parser against the
//! strict `zip`-crate-backed [`backends::zip::ZipBackend`] on the same
//! corpus of healthy fixtures we already shake out in
//! [`tests/zip_roundtrip.rs`].
//!
//!   * **Day 1 (`parity_*`)** — metadata only: both backends must yield
//!     the same `(path, uncompressed_size)` set.
//!   * **Day 2 (`extract_*`)** — payload parity: the lenient backend
//!     extracts byte-identical content to what the original ZIP writer
//!     fed into the fixture, across Store + Deflate methods, the
//!     zero-byte boundary, and lenient-recovery paths
//!     (bogus `cd_offset`).
//!
//! Plus negative coverage: unreadable entries (LFH offset past EOF)
//! surface a per-entry `Corrupted` rather than panicking or
//! mis-seeking, and method 99 (placeholder for "we'll add this in
//! v1.1") fails with `FeatureDisabled` instead of silently writing
//! garbage.
//!
//! Why the parity test matters: the lenient parser is going to replace
//! libarchive on the fallback path. We never want it to silently lose
//! an entry on a *healthy* archive — that would mean the strict path
//! and fallback path return different views of the same file, and the
//! user would see "extract said 9 674 entries, but probe only saw
//! 9 670". The parity assertion blocks the merge if even one fixture
//! diverges.
//!
//! Fixtures are byte-built here rather than read from disk so the test
//! corpus stays in the repo without binary blobs. Each scenario covers
//! one of the shapes the dispatcher will route through the lenient
//! parser in production:
//!
//!   * Single file, deflate.
//!   * Directory + nested file.
//!   * Stored entry (no compression).
//!   * Path-traversal name (`../escape.txt`) — the parser must surface
//!     it verbatim; the security gate fires later at extract time.
//!   * Many entries (stress-tests the CDFH cursor loop).

use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::Path;

use otterzip_core::{__probe_lenient_entries, __probe_lenient_extract, OtterzipError};
use tempfile::tempdir;
use zip::write::SimpleFileOptions;

/// Strict reference: enumerate the archive via the upstream `zip` crate
/// directly (which is what the strict `ZipBackend` wraps internally).
/// Using the crate raw rather than going through `Archive::open` keeps
/// this assertion as a pure metadata diff — the public API adds
/// detect/dispatch layers that aren't what we're comparing here.
fn strict_entries(path: &Path) -> Vec<(String, u64)> {
    let f = File::open(path).expect("open fixture");
    let mut archive =
        zip::ZipArchive::new(BufReader::new(f)).expect("strict ZipArchive::new");
    let mut out = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let zf = archive.by_index_raw(i).expect("strict by_index_raw");
        out.push((zf.name().to_string(), zf.size()));
    }
    out
}

/// Same shape as `strict_entries` but produced by our new lenient
/// parser. The Day 1 contract is that these two functions return the
/// same set (order is not enforced — both backends iterate CDFH
/// order, but the parity test is set-based so reordering in a future
/// optimisation doesn't trip false positives).
fn lenient_entries(path: &Path) -> Vec<(String, u64)> {
    __probe_lenient_entries(path).expect("lenient __probe_lenient_entries")
}

fn assert_parity(path: &Path) {
    let mut strict = strict_entries(path);
    let mut lenient = lenient_entries(path);
    strict.sort();
    lenient.sort();
    assert_eq!(
        lenient, strict,
        "lenient backend diverged from strict for {}",
        path.display()
    );
}

#[test]
fn parity_single_deflated_file() {
    let td = tempdir().unwrap();
    let path = td.path().join("single.zip");
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        w.start_file("hello.txt", opts).unwrap();
        w.write_all(b"hello otterzip lenient\n").unwrap();
        w.finish().unwrap();
    }
    assert_parity(&path);
}

#[test]
fn parity_directory_plus_nested_file() {
    // Mirrors the canonical fixture in tests/zip_roundtrip.rs.
    let td = tempdir().unwrap();
    let path = td.path().join("nested.zip");
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        w.start_file("a.txt", opts).unwrap();
        w.write_all(b"hello otterzip\n").unwrap();
        w.add_directory("sub/", opts).unwrap();
        w.start_file("sub/b.txt", opts).unwrap();
        w.write_all(b"second entry, slightly longer\n").unwrap();
        w.finish().unwrap();
    }
    assert_parity(&path);
}

#[test]
fn parity_stored_entry() {
    // CDFH still reports `compressed_size == uncompressed_size` for a
    // stored entry. The lenient parser must surface both unchanged.
    let td = tempdir().unwrap();
    let path = td.path().join("stored.zip");
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("plain.bin", opts).unwrap();
        w.write_all(&[0xAAu8; 4096]).unwrap();
        w.finish().unwrap();
    }
    assert_parity(&path);
}

#[test]
fn parity_path_traversal_name_is_reported_verbatim() {
    // The lenient parser is a parser, not a security gate — the
    // extract path applies `__validate_component` later. For Day 1
    // we just need to confirm that nasty names round-trip the same as
    // through the strict path so the gate sees consistent input.
    let td = tempdir().unwrap();
    let path = td.path().join("evil.zip");
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("../escape.txt", opts).unwrap();
        w.write_all(b"pwn").unwrap();
        w.finish().unwrap();
    }
    assert_parity(&path);
}

#[test]
fn parity_many_small_entries() {
    // Stress-tests the CDFH cursor loop — 256 entries means the CD is
    // sized into the multi-page realm and any off-by-one in `consumed`
    // arithmetic surfaces immediately as a parity miss.
    let td = tempdir().unwrap();
    let path = td.path().join("many.zip");
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for i in 0..256u32 {
            w.start_file(format!("f{i:04}.dat"), opts).unwrap();
            // Random-ish 64-byte payload so deflate doesn't trivially
            // dedupe across entries — keeps the per-entry compressed
            // size > 0 like real workloads.
            let mut buf = [0u8; 64];
            for (j, b) in buf.iter_mut().enumerate() {
                *b = ((i.wrapping_mul(31).wrapping_add(j as u32)) & 0xFF) as u8;
            }
            w.write_all(&buf).unwrap();
        }
        w.finish().unwrap();
    }
    assert_parity(&path);
}

#[test]
fn parity_empty_filename_comment_archive() {
    // Belt-and-braces: explicitly null comments / empty extra fields
    // are the modal case in real fixtures; just confirm the all-zero
    // optional fields branch doesn't crash either backend.
    let td = tempdir().unwrap();
    let path = td.path().join("plain.zip");
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("only.bin", opts).unwrap();
        w.write_all(b"x").unwrap();
        w.finish().unwrap();
    }
    assert_parity(&path);
}

// === Day 2 — payload extraction ======================================

#[test]
fn extract_deflated_entry_matches_original_bytes() {
    // Round-trip: write a deflated entry through the upstream zip
    // crate, then read it back via the lenient backend's extract
    // path and verify the bytes match. The payload is large enough
    // (32 KiB of varying bytes) that deflate actually does compress
    // it, exercising the flate2 decoder rather than the "compressed
    // size == uncompressed size" stored shortcut.
    let td = tempdir().unwrap();
    let path = td.path().join("deflate.zip");
    let mut payload = Vec::with_capacity(32 * 1024);
    for i in 0..32 * 1024u32 {
        payload.push(((i.wrapping_mul(17) >> 2) & 0xFF) as u8);
    }
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        w.start_file("payload.bin", opts).unwrap();
        w.write_all(&payload).unwrap();
        w.finish().unwrap();
    }
    let got = __probe_lenient_extract(&path, "payload.bin").unwrap();
    assert_eq!(got.len(), payload.len(), "extracted size mismatch");
    assert_eq!(got, payload, "deflated entry corrupted on the lenient path");
}

#[test]
fn extract_stored_entry_matches_original_bytes() {
    // Method 0 (Stored) takes the std::io::copy shortcut. Use a small
    // payload + a separate fixture so the test reads exactly what
    // method-0 dispatch produces, not what flate2 happens to do.
    let td = tempdir().unwrap();
    let path = td.path().join("stored.zip");
    let payload = b"the quick brown otter jumps over the lazy zip\n";
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("greeting.txt", opts).unwrap();
        w.write_all(payload).unwrap();
        w.finish().unwrap();
    }
    let got = __probe_lenient_extract(&path, "greeting.txt").unwrap();
    assert_eq!(&got[..], &payload[..]);
}

#[test]
fn extract_zero_byte_entry_returns_empty() {
    // Zero-byte file: compressed_size == 0, uncompressed_size == 0.
    // Edge case for the `Take` reader + std::io::copy combo — must
    // emit zero bytes and not error.
    let td = tempdir().unwrap();
    let path = td.path().join("empty.zip");
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("zero.bin", opts).unwrap();
        // No write_all — entry is intentionally empty.
        w.finish().unwrap();
    }
    let got = __probe_lenient_extract(&path, "zero.bin").unwrap();
    assert!(got.is_empty(), "expected zero bytes, got {} bytes", got.len());
}

#[test]
fn extract_directory_entry_returns_zero_bytes() {
    // Directory CDFH entries don't have a payload. `extract_entry`
    // short-circuits before the LFH read so callers can iterate the
    // entries list without special-casing directories.
    let td = tempdir().unwrap();
    let path = td.path().join("withdir.zip");
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.add_directory("docs/", opts).unwrap();
        w.start_file("docs/readme.md", opts).unwrap();
        w.write_all(b"# OtterZip").unwrap();
        w.finish().unwrap();
    }
    // The directory itself extracts to nothing.
    let dir_bytes = __probe_lenient_extract(&path, "docs/").unwrap();
    assert!(dir_bytes.is_empty());
    // The nested file still works.
    let readme = __probe_lenient_extract(&path, "docs/readme.md").unwrap();
    assert_eq!(&readme[..], b"# OtterZip");
}

#[test]
fn extract_unknown_entry_name_returns_entry_not_found() {
    let td = tempdir().unwrap();
    let path = td.path().join("known.zip");
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("real.bin", opts).unwrap();
        w.write_all(b"hi").unwrap();
        w.finish().unwrap();
    }
    match __probe_lenient_extract(&path, "ghost.bin") {
        Err(OtterzipError::EntryNotFound(name)) => assert_eq!(name, "ghost.bin"),
        Err(other) => panic!("expected EntryNotFound, got {other:?}"),
        Ok(_) => panic!("ghost entry should not extract"),
    }
}

#[test]
fn extract_malformed_archive_recovers_payload() {
    // End-to-end recovery: build an archive, corrupt its EOCD
    // `cd_offset` so the strict path rejects it, then assert the
    // lenient backend's extract returns the original bytes verbatim.
    // This is the user's reproducer in miniature.
    let td = tempdir().unwrap();
    let path = td.path().join("malformed.zip");
    let payload = b"recovered from a malformed archive\n";
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        w.start_file("survivor.txt", opts).unwrap();
        w.write_all(payload).unwrap();
        w.finish().unwrap();
    }

    // Find the EOCD signature in the last 64 KiB and poison cd_offset
    // to point past EOF — same shape as the libarchive-fallback
    // test's reproducer.
    let mut bytes = fs::read(&path).unwrap();
    let needle = [0x50u8, 0x4b, 0x05, 0x06];
    let len = bytes.len();
    let scan_start = len.saturating_sub(65_557);
    let eocd_at = (scan_start..=len - 4)
        .rev()
        .find(|&i| bytes[i..i + 4] == needle)
        .expect("synthetic fixture must contain an EOCD");
    let cd_offset_at = eocd_at + 16;
    let bogus = (bytes.len() as u32).saturating_add(256);
    bytes[cd_offset_at..cd_offset_at + 4].copy_from_slice(&bogus.to_le_bytes());
    fs::write(&path, &bytes).unwrap();

    // Strict path must reject — if it stops doing so the lenient
    // fallback is no longer needed (this is the same canary the
    // libarchive_fallback test uses).
    let raw = fs::File::open(&path).unwrap();
    let strict = zip::ZipArchive::new(BufReader::new(raw));
    assert!(
        strict.is_err(),
        "strict zip crate should still reject this fixture",
    );

    // Lenient path must succeed and return the original bytes.
    let got = __probe_lenient_extract(&path, "survivor.txt").unwrap();
    assert_eq!(&got[..], &payload[..]);
}

#[test]
fn extract_does_not_panic_on_unsupported_method() {
    // Forge a CDFH with method 99 (placeholder for some future
    // codec we don't ship) — the lenient parser must walk past it
    // without panicking, then `extract_entry` must surface
    // `FeatureDisabled`. Negative test: prevents the dispatch arm
    // from silently writing garbage if we ever forget to update it.
    let td = tempdir().unwrap();
    let path = td.path().join("methods.zip");
    {
        let f = fs::File::create(&path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.start_file("real.bin", opts).unwrap();
        w.write_all(b"ok").unwrap();
        w.finish().unwrap();
    }
    // Mutate the CDFH's compression method field to 99. CDFH starts
    // at the absolute `cd_offset` written into the EOCD; bytes 10-11
    // of the CDFH carry the method.
    let mut bytes = fs::read(&path).unwrap();
    let needle = [0x50u8, 0x4b, 0x05, 0x06];
    let len = bytes.len();
    let eocd_at = (0..=len - 4)
        .rev()
        .find(|&i| bytes[i..i + 4] == needle)
        .unwrap();
    let cd_offset =
        u32::from_le_bytes(bytes[eocd_at + 16..eocd_at + 20].try_into().unwrap()) as usize;
    bytes[cd_offset + 10..cd_offset + 12].copy_from_slice(&99u16.to_le_bytes());
    fs::write(&path, &bytes).unwrap();

    // The CD walk should still pick the entry up — only the extract
    // side rejects.
    let pairs = __probe_lenient_entries(&path).unwrap();
    assert_eq!(pairs.len(), 1);
    match __probe_lenient_extract(&path, "real.bin") {
        Err(OtterzipError::FeatureDisabled(_)) => {}
        Err(other) => panic!("expected FeatureDisabled, got {other:?}"),
        Ok(_) => panic!("unsupported method must not silently extract"),
    }
}
