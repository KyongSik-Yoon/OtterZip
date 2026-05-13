//! Lenient ZIP backend — Day 1 regression tests.
//!
//! Compares the new in-tree [`backends::lenient_zip`] parser against the
//! strict `zip`-crate-backed [`backends::zip::ZipBackend`] on the same
//! corpus of healthy fixtures we already shake out in
//! [`tests/zip_roundtrip.rs`]. Day 1 is metadata-only: both backends
//! must yield the same `(path, uncompressed_size)` set. Per-entry
//! payload parity lands when Day 2 wires LFH decompression.
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

use otterzip_core::__probe_lenient_entries;
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
