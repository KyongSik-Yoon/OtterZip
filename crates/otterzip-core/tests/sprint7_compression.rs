//! Phase 7+ option Y — PR-F2 acceptance tests for the next-generation
//! compression family: ZSTD / TAR.ZST / LZ4 / TAR.LZ4.
//!
//! Coverage matrix per `docs/05-build/phase-7-plus-plan.md` PR-F2:
//! 1. Round-trip extract for `.zst` and `.lz4` single streams.
//! 2. Round-trip extract for `.tar.zst` and `.tar.lz4` (multi-entry).
//! 3. Compression-ratio sanity — `.tar.zst` <= `.tar.gz` on the same
//!    text payload (regression guard against accidentally feeding the
//!    encoder a `Compression::default()` that's worse than gzip).
//! 4. Format detection — magic bytes and extension hints route to the
//!    expected `ArchiveFormat` variants.
//!
//! The performance gate from the plan ("ZSTD level 3 extract >= 200
//! MiB/s") is intentionally _not_ enforced here — single-thread MiB/s
//! is hardware-dependent and would either flake on CI or require a
//! benchmark harness. The criterion bench in `crates/otterzip-bench`
//! is the right home for that number.

use std::fs;
use std::io::Write;
use std::path::Path;

use otterzip_core::{Archive, ArchiveFormat, ExtractOptions, OpenMode, ProgressSink};
use tempfile::tempdir;

/// `extract_all` requires a concrete `S: ProgressSink` even when the
/// caller passes `None`; this no-op satisfies that bound without
/// pulling in the crate-private `NoopSink`.
struct NullSink;
impl ProgressSink for NullSink {
    fn update(&mut self, _: &otterzip_core::Progress) -> bool {
        true
    }
}

/// Mixed ASCII + binary payload large enough (~16 KiB) that gzip vs
/// zstd actually have something to chew on, small enough that the
/// tests stay snappy.
fn fixture_payload() -> Vec<u8> {
    let mut v = Vec::with_capacity(16 * 1024);
    for i in 0..16 * 1024u32 {
        v.push((i & 0xFF) as u8);
        if i % 17 == 0 {
            v.extend_from_slice(b"OtterZip F2 ZSTD/LZ4 acceptance ");
        }
    }
    v
}

// --- single-stream encoders for fixtures ----------------------------------

fn write_zstd(path: &Path, data: &[u8]) {
    let f = fs::File::create(path).unwrap();
    // Default level (3) — keeps the test fast and matches the plan's
    // performance target reference point.
    let mut enc = zstd::stream::write::Encoder::new(f, 3).unwrap().auto_finish();
    enc.write_all(data).unwrap();
}

fn write_lz4(path: &Path, data: &[u8]) {
    let f = fs::File::create(path).unwrap();
    let mut enc = lz4_flex::frame::FrameEncoder::new(f);
    enc.write_all(data).unwrap();
    enc.finish().unwrap();
}

fn write_tar_zst(path: &Path, members: &[(&str, &[u8])]) {
    let f = fs::File::create(path).unwrap();
    let zenc = zstd::stream::write::Encoder::new(f, 3).unwrap().auto_finish();
    let mut tarball = tar::Builder::new(zenc);
    for (name, data) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tarball.append_data(&mut header, name, *data).unwrap();
    }
    tarball.finish().unwrap();
}

fn write_tar_lz4(path: &Path, members: &[(&str, &[u8])]) {
    let f = fs::File::create(path).unwrap();
    let lenc = lz4_flex::frame::FrameEncoder::new(f);
    let mut tarball = tar::Builder::new(lenc);
    for (name, data) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tarball.append_data(&mut header, name, *data).unwrap();
    }
    let lenc = tarball.into_inner().unwrap();
    lenc.finish().unwrap();
}

fn write_tar_gz(path: &Path, members: &[(&str, &[u8])]) {
    let f = fs::File::create(path).unwrap();
    let genc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut tarball = tar::Builder::new(genc);
    for (name, data) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tarball.append_data(&mut header, name, *data).unwrap();
    }
    let genc = tarball.into_inner().unwrap();
    genc.finish().unwrap();
}

fn extract_single(archive_path: &Path, dest_dir: &Path) -> Vec<u8> {
    let archive = Archive::open(archive_path, OpenMode::Read).expect("open");
    let opts = ExtractOptions {
        destination: dest_dir.to_path_buf(),
        ..ExtractOptions::default()
    };
    archive.extract_all::<NullSink>(&opts, None).expect("extract_all");
    let entries: Vec<_> = fs::read_dir(dest_dir).unwrap().filter_map(|e| e.ok()).collect();
    assert_eq!(entries.len(), 1, "expected exactly one extracted file");
    fs::read(entries[0].path()).unwrap()
}

// --- single-stream round-trips --------------------------------------------

#[test]
fn zstd_single_stream_roundtrip() {
    let payload = fixture_payload();
    let td = tempdir().unwrap();
    let src = td.path().join("payload.txt.zst");
    write_zstd(&src, &payload);
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();

    let archive = Archive::open(&src, OpenMode::Read).expect("open .zst");
    assert_eq!(archive.format(), ArchiveFormat::Zstd);
    let extracted = extract_single(&src, &dest);
    assert_eq!(extracted, payload, "zstd round-trip mismatch");
}

#[test]
fn lz4_single_stream_roundtrip() {
    let payload = fixture_payload();
    let td = tempdir().unwrap();
    let src = td.path().join("payload.txt.lz4");
    write_lz4(&src, &payload);
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();

    let archive = Archive::open(&src, OpenMode::Read).expect("open .lz4");
    assert_eq!(archive.format(), ArchiveFormat::Lz4);
    let extracted = extract_single(&src, &dest);
    assert_eq!(extracted, payload, "lz4 round-trip mismatch");
}

// --- tar wrapper round-trips ----------------------------------------------

fn extract_tar_and_verify(src: &Path, expected: &[(&str, &[u8])]) {
    let td_out = tempdir().unwrap();
    let dest = td_out.path();
    let archive = Archive::open(src, OpenMode::Read).expect("open tar variant");
    let opts = ExtractOptions {
        destination: dest.to_path_buf(),
        ..ExtractOptions::default()
    };
    archive.extract_all::<NullSink>(&opts, None).expect("extract_all");

    for (name, expected_bytes) in expected {
        let p = dest.join(name);
        let actual = fs::read(&p)
            .unwrap_or_else(|_| panic!("missing extracted member: {} at {:?}", name, p));
        assert_eq!(actual, *expected_bytes, "byte mismatch for {}", name);
    }
}

#[test]
fn tar_zst_roundtrip_multi_entry() {
    let td = tempdir().unwrap();
    let src = td.path().join("bundle.tar.zst");
    let members: &[(&str, &[u8])] = &[
        ("a.txt", b"alpha entry contents\n"),
        ("dir/b.txt", b"nested beta entry\n"),
        ("dir/c.bin", &[0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
    ];
    write_tar_zst(&src, members);

    let archive = Archive::open(&src, OpenMode::Read).expect("open .tar.zst");
    assert_eq!(archive.format(), ArchiveFormat::TarZst);
    extract_tar_and_verify(&src, members);
}

#[test]
fn tar_lz4_roundtrip_multi_entry() {
    let td = tempdir().unwrap();
    let src = td.path().join("bundle.tar.lz4");
    let members: &[(&str, &[u8])] = &[
        ("first.txt", b"the first member\n"),
        ("nested/second.txt", b"member in a directory\n"),
    ];
    write_tar_lz4(&src, members);

    let archive = Archive::open(&src, OpenMode::Read).expect("open .tar.lz4");
    assert_eq!(archive.format(), ArchiveFormat::TarLz4);
    extract_tar_and_verify(&src, members);
}

// --- compression-ratio sanity --------------------------------------------

#[test]
fn tar_zst_smaller_or_equal_to_tar_gz_on_text_payload() {
    // Highly compressible text payload — repeating strings.
    let mut payload = Vec::with_capacity(64 * 1024);
    for _ in 0..2048 {
        payload.extend_from_slice(b"the quick brown fox jumps over the lazy dog\n");
    }
    let members: &[(&str, &[u8])] = &[("payload.txt", &payload[..])];

    let td = tempdir().unwrap();
    let zst_path = td.path().join("p.tar.zst");
    let gz_path = td.path().join("p.tar.gz");
    write_tar_zst(&zst_path, members);
    write_tar_gz(&gz_path, members);

    let zst_size = fs::metadata(&zst_path).unwrap().len();
    let gz_size = fs::metadata(&gz_path).unwrap().len();

    // Plan acceptance: tar.zst <= tar.gz on the same payload (level 3 vs
    // gzip default). On this text fixture zstd consistently wins.
    assert!(
        zst_size <= gz_size,
        "tar.zst ({} B) should compress no worse than tar.gz ({} B) for text",
        zst_size,
        gz_size,
    );
}

// --- detection contract ---------------------------------------------------

#[test]
fn detect_routes_zst_to_zstd_variant() {
    let payload = fixture_payload();
    let td = tempdir().unwrap();
    let src = td.path().join("payload.zst");
    write_zstd(&src, &payload);
    assert_eq!(otterzip_core::detect(&src).unwrap(), ArchiveFormat::Zstd);
}

#[test]
fn detect_routes_tar_zst_via_double_extension() {
    let td = tempdir().unwrap();
    let src = td.path().join("bundle.tar.zst");
    let members: &[(&str, &[u8])] = &[("only.txt", b"x")];
    write_tar_zst(&src, members);
    assert_eq!(otterzip_core::detect(&src).unwrap(), ArchiveFormat::TarZst);
}

#[test]
fn detect_routes_lz4_to_lz4_variant() {
    let td = tempdir().unwrap();
    let src = td.path().join("payload.lz4");
    write_lz4(&src, b"some bytes");
    assert_eq!(otterzip_core::detect(&src).unwrap(), ArchiveFormat::Lz4);
}

#[test]
fn detect_routes_tar_lz4_via_double_extension() {
    let td = tempdir().unwrap();
    let src = td.path().join("bundle.tar.lz4");
    let members: &[(&str, &[u8])] = &[("only.txt", b"y")];
    write_tar_lz4(&src, members);
    assert_eq!(otterzip_core::detect(&src).unwrap(), ArchiveFormat::TarLz4);
}
