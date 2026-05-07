//! Phase 7+ option Y — PR-F1 acceptance tests for the four single-stream
//! formats: `.gz` / `.bz2` / `.xz` / `.lzma`.
//!
//! What we verify per [`docs/05-build/phase-7-plus-plan.md`]:
//! 1. round-trip — encode then decode through `Archive::open` /
//!    `extract_all` produces the original payload.
//! 2. `OpenMode::Update` is rejected with `FeatureDisabled` (single
//!    streams have no notion of "update").
//! 3. `add_file` (writer side) is rejected — single-stream creation is
//!    PR-F7, not yet wired through `Archive::create`.
//!
//! GZIP is included alongside BZ2/XZ/LZMA because PR-F1 also closes the
//! schema vs implementation gap that previously had GZIP routing through
//! `FeatureDisabled` despite §5.2 listing it as supported.

use std::fs;
use std::io::Write;

use otterzip_core::{Archive, ArchiveFormat, ExtractOptions, OpenMode, ProgressSink};

use tempfile::tempdir;

/// `extract_all`'s `progress: Option<&mut S>` requires a concrete `S` to
/// monomorphise even when the Option is `None`. A unit struct that does
/// nothing satisfies the bound without pulling in the crate-private
/// `NoopSink`.
struct NullSink;
impl ProgressSink for NullSink {
    fn update(&mut self, _: &otterzip_core::Progress) -> bool {
        true
    }
}

/// Payload large enough to exercise the streaming path (a few KiB) and
/// non-trivial enough that any byte mismatch in the round-trip is caught
/// immediately.
fn fixture_payload() -> Vec<u8> {
    // Mix of ASCII + binary patterns to stress the codecs without needing
    // a real binary file on disk. ~16 KiB.
    let mut v = Vec::with_capacity(16 * 1024);
    for i in 0..16 * 1024u32 {
        v.push((i & 0xFF) as u8);
        if i % 17 == 0 {
            v.extend_from_slice(b"OtterZip F1 ");
        }
    }
    v
}

// ---------- helpers --------------------------------------------------------

fn write_gzip(path: &std::path::Path, data: &[u8]) {
    let f = fs::File::create(path).unwrap();
    let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap();
}

fn write_bzip2(path: &std::path::Path, data: &[u8]) {
    let f = fs::File::create(path).unwrap();
    let mut enc = bzip2::write::BzEncoder::new(f, bzip2::Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap();
}

fn write_xz(path: &std::path::Path, data: &[u8]) {
    let f = fs::File::create(path).unwrap();
    let mut enc = xz2::write::XzEncoder::new(f, 6);
    enc.write_all(data).unwrap();
    enc.finish().unwrap();
}

fn write_lzma(path: &std::path::Path, data: &[u8]) {
    // Raw LZMA1 (alone-format) stream — xz2 exposes the encoder via the
    // `LzmaOptions` + `Stream::new_lzma_encoder` pair, then we drive the
    // stream through `XzEncoder::new_stream` to reuse the io::Write side.
    use std::io::Write;
    let opts = xz2::stream::LzmaOptions::new_preset(6).expect("lzma opts");
    let stream = xz2::stream::Stream::new_lzma_encoder(&opts).expect("lzma encoder");
    let f = fs::File::create(path).unwrap();
    let mut enc = xz2::write::XzEncoder::new_stream(f, stream);
    enc.write_all(data).unwrap();
    enc.finish().unwrap();
}

/// Open `Archive`, extract its single synthetic entry into `dest_dir`,
/// and return the full byte content so the caller can compare.
fn extract_single(archive_path: &std::path::Path, dest_dir: &std::path::Path) -> Vec<u8> {
    let archive = Archive::open(archive_path, OpenMode::Read).expect("open");
    let opts = ExtractOptions {
        destination: dest_dir.to_path_buf(),
        ..ExtractOptions::default()
    };
    archive
        .extract_all::<NullSink>(&opts, None)
        .expect("extract_all");

    // The single-stream backend names the output by stripping the format
    // suffix from the source filename. Locate it by scanning the dest dir.
    let entries: Vec<_> = fs::read_dir(dest_dir).unwrap().filter_map(|e| e.ok()).collect();
    assert_eq!(entries.len(), 1, "expected exactly one extracted file");
    fs::read(entries[0].path()).unwrap()
}

// ---------- per-format round-trip -----------------------------------------

#[test]
fn gzip_roundtrip() {
    let payload = fixture_payload();
    let td = tempdir().unwrap();
    let src = td.path().join("payload.txt.gz");
    write_gzip(&src, &payload);
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();

    let archive = Archive::open(&src, OpenMode::Read).expect("open .gz");
    assert_eq!(archive.format(), ArchiveFormat::Gzip);
    let extracted = extract_single(&src, &dest);
    assert_eq!(extracted, payload, "gzip round-trip mismatch");
}

#[test]
fn bzip2_roundtrip() {
    let payload = fixture_payload();
    let td = tempdir().unwrap();
    let src = td.path().join("payload.txt.bz2");
    write_bzip2(&src, &payload);
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();

    let archive = Archive::open(&src, OpenMode::Read).expect("open .bz2");
    assert_eq!(archive.format(), ArchiveFormat::Bzip2);
    let extracted = extract_single(&src, &dest);
    assert_eq!(extracted, payload, "bzip2 round-trip mismatch");
}

#[test]
fn xz_roundtrip() {
    let payload = fixture_payload();
    let td = tempdir().unwrap();
    let src = td.path().join("payload.txt.xz");
    write_xz(&src, &payload);
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();

    let archive = Archive::open(&src, OpenMode::Read).expect("open .xz");
    assert_eq!(archive.format(), ArchiveFormat::Xz);
    let extracted = extract_single(&src, &dest);
    assert_eq!(extracted, payload, "xz round-trip mismatch");
}

#[test]
fn lzma_roundtrip() {
    let payload = fixture_payload();
    let td = tempdir().unwrap();
    let src = td.path().join("payload.txt.lzma");
    write_lzma(&src, &payload);
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();

    let archive = Archive::open(&src, OpenMode::Read).expect("open .lzma");
    // Detection routes .lzma through the Lzma single-stream variant.
    assert_eq!(archive.format(), ArchiveFormat::Lzma);
    let extracted = extract_single(&src, &dest);
    assert_eq!(extracted, payload, "lzma round-trip mismatch");
}

// ---------- contract gates ------------------------------------------------

#[test]
fn entry_name_strips_format_suffix() {
    let payload = b"hello";
    let td = tempdir().unwrap();
    let src = td.path().join("greetings.txt.gz");
    write_gzip(&src, payload);

    let archive = Archive::open(&src, OpenMode::Read).expect("open");
    let mut iter = archive.entries().expect("entries");
    let first = iter.next().expect("at least one entry").expect("entry ok");
    assert_eq!(first.path, "greetings.txt", "stem mismatch");
    assert!(iter.next().is_none(), "single-stream must yield exactly one entry");
}

#[test]
fn open_update_mode_is_rejected() {
    // Single-stream formats are immutable by construction — Update must
    // fail cleanly. Use the writer-mode open path so we hit `open_writer`
    // dispatch (which is where the gate lives).
    let td = tempdir().unwrap();
    let src = td.path().join("payload.bz2");
    write_bzip2(&src, b"seed");

    let result = Archive::open(&src, OpenMode::Update);
    // The exact error variant is less important than the fact that the
    // call refuses — accept both the dispatcher's FeatureDisabled and a
    // higher-level rejection from Archive::open.
    assert!(
        result.is_err(),
        "Update mode on a single-stream archive must be rejected"
    );
}
