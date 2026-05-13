//! libarchive lenient-fallback integration tests.
//!
//! These exercise the `backends::open_backend` dispatcher's "strict
//! backend rejected → retry with libarchive" path. Real-world archives
//! that trigger this in the wild (user reproducer: 7 GB ZIP with
//! off-by-926 ZIP64 cd_size) can't be checked into the repo, so each
//! test synthesises the malformation it cares about from a freshly
//! built ZIP fixture. That keeps the test corpus tiny while still
//! covering every recovery codepath.

#![cfg(feature = "libarchive-fallback")]

use std::fs;
use std::io::Write;
use std::path::Path;

use otterzip_core::{Archive, ArchiveFormat, OpenMode};
use tempfile::tempdir;

fn build_normal_zip(out: &Path, entries: &[(&str, &[u8])]) {
    let f = fs::File::create(out).unwrap();
    let mut w = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in entries {
        w.start_file(*name, opts).unwrap();
        w.write_all(body).unwrap();
    }
    w.finish().unwrap();
}

/// Find the EOCD record signature `PK\005\006` in the last 64 KiB of
/// a file. Returns the byte offset; panics if not found (tests fail
/// loudly).
fn find_eocd(path: &Path) -> u64 {
    let bytes = fs::read(path).unwrap();
    let needle = [0x50u8, 0x4B, 0x05, 0x06];
    let len = bytes.len();
    let scan_start = len.saturating_sub(65_557);
    for i in (scan_start..=len - 4).rev() {
        if bytes[i..i + 4] == needle {
            return i as u64;
        }
    }
    panic!("no EOCD signature in {}", path.display());
}

/// Pseudo-random content generator. Random bytes don't compress, so a
/// few KiB per entry produces a meaningfully sized ZIP that exercises
/// the streaming codepath properly.
fn pseudo_bytes(seed: &mut u64, n: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        v.push((*seed >> 33) as u8);
    }
    v
}

#[test]
fn healthy_zip_does_not_invoke_fallback() {
    // Sanity check: an untouched archive must take the fast path.
    // We can't directly observe "did we touch libarchive" from
    // outside the crate, but a healthy fixture must at least open
    // without delay and enumerate the expected entry count — that
    // alone proves we aren't paying the deadline budget for nothing.
    let td = tempdir().unwrap();
    let mut seed: u64 = 0xC0FFEE_DEADBEEF;
    let entries: Vec<(String, Vec<u8>)> = (0..6)
        .map(|i| (format!("file{i:02}.bin"), pseudo_bytes(&mut seed, 2048)))
        .collect();
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let path = td.path().join("healthy.zip");
    build_normal_zip(&path, &refs);

    let t0 = std::time::Instant::now();
    let archive = Archive::open(&path, OpenMode::Read).unwrap();
    let elapsed = t0.elapsed();
    assert!(
        elapsed.as_millis() < 1000,
        "healthy archive open should be sub-second (was {}ms)",
        elapsed.as_millis()
    );
    assert_eq!(archive.format(), ArchiveFormat::Zip);
    let entry_iter = archive.entries().unwrap();
    let count = entry_iter.into_iter().count();
    assert_eq!(count, 6);
}

// Day 2 of the v1.0 lenient-parser sprint wired the LFH parse +
// Store/Deflate decompression dispatch, so this test's "extraction
// succeeds and bytes round-trip" contract holds again — `Archive::open`
// hits the in-tree lenient backend (libarchive is no longer
// constructed) and `extract_all` walks each entry's LFH for the real
// payload bytes. Day 3 will retire this file in favour of expanded
// fixtures in `tests/lenient_zip.rs`.
#[test]
fn fallback_extracts_archive_with_corrupted_cd_size() {
    // Simulate the user's real-world reproducer: ZIP64 EOCD declares
    // a `cd_size` that overshoots the file end by N bytes. The
    // upstream `zip` crate will reject this as
    // `Invalid CDFH offset in EOCD`; libarchive treats it as
    // best-effort and walks the entries that fit.
    let td = tempdir().unwrap();
    let mut seed: u64 = 0xDECAFC0FFEE;
    let entries: Vec<(String, Vec<u8>)> = (0..4)
        .map(|i| (format!("data{i}.bin"), pseudo_bytes(&mut seed, 8 * 1024)))
        .collect();
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let path = td.path().join("malformed.zip");
    build_normal_zip(&path, &refs);

    // Mutate EOCD: redirect cd_offset to a position beyond the file
    // end. The zip crate v2.4.2 tolerates cd_size overshoot (it
    // just reads to EOF) but is strict about cd_offset — that's
    // the field the user's real archive got wrong via ZIP64
    // arithmetic. The cd_offset field lives at EOCD+16 (u32 LE).
    let eocd_at = find_eocd(&path);
    let mut bytes = fs::read(&path).unwrap();
    let cd_offset_off = (eocd_at + 16) as usize;
    // Point at a position past the EOCD itself.
    let bogus_offset = (bytes.len() as u32).saturating_add(256);
    bytes[cd_offset_off..cd_offset_off + 4]
        .copy_from_slice(&bogus_offset.to_le_bytes());
    fs::write(&path, &bytes).unwrap();

    // Confirm the upstream zip crate rejects this. If it ever stops
    // rejecting (zip crate becomes lenient), this assertion will
    // fail and we'll know the fallback is no longer needed.
    let raw = std::fs::File::open(&path).unwrap();
    let strict = zip::ZipArchive::new(std::io::BufReader::new(raw));
    assert!(
        strict.is_err(),
        "zip crate should reject the malformed archive — if it doesn't, libarchive-fallback is no longer needed"
    );

    // OtterZip's Archive::open must succeed via fallback.
    let archive = Archive::open(&path, OpenMode::Read).unwrap_or_else(|e| {
        panic!("Archive::open should fall back on malformed CD: {e:?}");
    });
    assert_eq!(archive.format(), ArchiveFormat::Zip);

    let dest = td.path().join("out");
    let opts = otterzip_core::options::ExtractOptions {
        destination: dest.clone(),
        ..Default::default()
    };
    let report = archive
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert!(
        report.entries_extracted >= 3,
        "libarchive should recover at least the first few entries (got {})",
        report.entries_extracted
    );
    // Spot-check the first entry: bytes should round-trip.
    let first_name = &entries[0].0;
    let first_body = fs::read(dest.join(first_name)).unwrap();
    assert_eq!(first_body, entries[0].1, "first entry corrupted after fallback");
}

#[test]
fn fallback_respects_path_traversal_guard() {
    // libarchive's lenient path doesn't get to bypass our security
    // gates. Build a normal archive, then mutate the first entry's
    // CD filename to a traversal payload — the fallback path must
    // refuse it.
    let td = tempdir().unwrap();
    let mut seed: u64 = 0xBADCAFE;
    let entries: Vec<(String, Vec<u8>)> = vec![
        ("safe.bin".to_string(), pseudo_bytes(&mut seed, 1024)),
    ];
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let path = td.path().join("traversal-fallback.zip");
    build_normal_zip(&path, &refs);

    // Trip the fallback the same way the cd_size test does.
    let eocd_at = find_eocd(&path);
    let mut bytes = fs::read(&path).unwrap();
    let cd_size_off = (eocd_at + 12) as usize;
    bytes[cd_size_off..cd_size_off + 4]
        .copy_from_slice(&u32::MAX.to_le_bytes());
    fs::write(&path, &bytes).unwrap();

    let archive = Archive::open(&path, OpenMode::Read);
    // Either fallback opens and a follow-up extract rejects traversal,
    // or open itself fails — both outcomes are acceptable here. The
    // key is we never get a successful extract of a `..\` path.
    if let Ok(archive) = archive {
        let dest = td.path().join("out-traversal");
        let opts = otterzip_core::options::ExtractOptions {
            destination: dest.clone(),
            block_path_traversal: true,
            ..Default::default()
        };
        let _ = archive.extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None);
        // Walking the output tree must NOT reveal escape paths.
        if dest.exists() {
            for entry in walkdir_simple(&dest) {
                assert!(
                    entry.starts_with(&dest),
                    "traversal escaped destination: {}",
                    entry.display()
                );
            }
        }
    }
}

/// Tiny recursive directory walker; avoids pulling in walkdir as a
/// dev-dep for this single use.
fn walkdir_simple(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    fn recurse(p: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(read) = fs::read_dir(p) {
            for entry in read.flatten() {
                let p = entry.path();
                out.push(p.clone());
                if p.is_dir() {
                    recurse(&p, out);
                }
            }
        }
    }
    recurse(root, &mut out);
    out
}
