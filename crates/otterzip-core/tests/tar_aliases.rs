//! Phase 7+ option Y — PR-F4 acceptance tests for TAR alias extensions.
//!
//! The detect layer already routed every alias except `.tlz` (legacy
//! GNU tar+LZMA1) before this PR. PR-F4 closes the gap and verifies
//! the full alias matrix end-to-end:
//!
//! | extension     | enum            | codec            |
//! |---------------|-----------------|------------------|
//! | `.tgz`        | `TarGz`         | gzip             |
//! | `.tbz` / `.tbz2` | `TarBz2`     | bzip2            |
//! | `.tlz`        | `TarXz` (path-aware dispatch) | LZMA1 |
//! | `.txz`        | `TarXz`         | LZMA2 / .xz      |
//! | `.tzst`       | `TarZst`        | zstd             |
//!
//! `.tar.lz4` has no traditional short alias, so it stays only under
//! the dotted form (covered in `sprint7_compression.rs`).

use std::fs;
use std::io::Write;
use std::path::Path;

use otterzip_core::{Archive, ArchiveFormat, ExtractOptions, OpenMode, ProgressSink};
use tempfile::tempdir;

struct NullSink;
impl ProgressSink for NullSink {
    fn update(&mut self, _: &otterzip_core::Progress) -> bool {
        true
    }
}

fn members_fixture() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("a.txt", b"alpha\n" as &[u8]),
        ("dir/b.txt", b"nested beta\n"),
    ]
}

fn assert_extracted(dest: &Path, members: &[(&str, &[u8])]) {
    for (name, data) in members {
        let p = dest.join(name);
        let actual = fs::read(&p).unwrap_or_else(|_| panic!("missing {} at {:?}", name, p));
        assert_eq!(actual, *data, "byte mismatch for {}", name);
    }
}

fn build_tar<W: Write>(writer: W, members: &[(&str, &[u8])]) -> W {
    let mut tarball = tar::Builder::new(writer);
    for (name, data) in members {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tarball.append_data(&mut header, name, *data).unwrap();
    }
    tarball.into_inner().unwrap()
}

// --- alias round-trips ----------------------------------------------------

#[test]
fn tgz_alias_extracts_as_tar_gz() {
    let td = tempdir().unwrap();
    let src = td.path().join("bundle.tgz");
    let f = fs::File::create(&src).unwrap();
    let genc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let members = members_fixture();
    let genc = build_tar(genc, &members);
    genc.finish().unwrap();

    let archive = Archive::open(&src, OpenMode::Read).expect("open .tgz");
    assert_eq!(archive.format(), ArchiveFormat::TarGz);
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    let opts = ExtractOptions {
        destination: dest.clone(),
        ..ExtractOptions::default()
    };
    archive.extract_all::<NullSink>(&opts, None).unwrap();
    assert_extracted(&dest, &members);
}

#[test]
fn tbz2_alias_extracts_as_tar_bz2() {
    let td = tempdir().unwrap();
    let src = td.path().join("bundle.tbz2");
    let f = fs::File::create(&src).unwrap();
    let benc = bzip2::write::BzEncoder::new(f, bzip2::Compression::default());
    let members = members_fixture();
    let benc = build_tar(benc, &members);
    benc.finish().unwrap();

    let archive = Archive::open(&src, OpenMode::Read).expect("open .tbz2");
    assert_eq!(archive.format(), ArchiveFormat::TarBz2);
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    let opts = ExtractOptions {
        destination: dest.clone(),
        ..ExtractOptions::default()
    };
    archive.extract_all::<NullSink>(&opts, None).unwrap();
    assert_extracted(&dest, &members);
}

#[test]
fn txz_alias_extracts_as_tar_xz_lzma2() {
    let td = tempdir().unwrap();
    let src = td.path().join("bundle.txz");
    let f = fs::File::create(&src).unwrap();
    let xenc = xz2::write::XzEncoder::new(f, 6);
    let members = members_fixture();
    let xenc = build_tar(xenc, &members);
    xenc.finish().unwrap();

    let archive = Archive::open(&src, OpenMode::Read).expect("open .txz");
    assert_eq!(archive.format(), ArchiveFormat::TarXz);
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    let opts = ExtractOptions {
        destination: dest.clone(),
        ..ExtractOptions::default()
    };
    archive.extract_all::<NullSink>(&opts, None).unwrap();
    assert_extracted(&dest, &members);
}

#[test]
fn tlz_alias_extracts_via_lzma1_dispatch() {
    // PR-F4 core gate: detect collapses .tlz onto the TarXz enum slot,
    // then `open_backend` notices the .tlz suffix and selects the LZMA1
    // codec instead of LZMA2. If the path-aware dispatch is missing
    // this test fails because XzDecoder can't read the alone-format
    // header.
    let td = tempdir().unwrap();
    let src = td.path().join("bundle.tlz");

    // Build an LZMA1-encoded tar by piping through a manual LZMA1 stream.
    let f = fs::File::create(&src).unwrap();
    let opts = xz2::stream::LzmaOptions::new_preset(6).expect("lzma opts");
    let stream = xz2::stream::Stream::new_lzma_encoder(&opts).expect("lzma encoder");
    let lenc = xz2::write::XzEncoder::new_stream(f, stream);
    let members = members_fixture();
    let lenc = build_tar(lenc, &members);
    lenc.finish().unwrap();

    let archive = Archive::open(&src, OpenMode::Read).expect("open .tlz");
    // The detect layer collapses .tlz onto the TarXz slot; the LZMA1
    // codec is picked path-aware inside the dispatcher.
    assert_eq!(archive.format(), ArchiveFormat::TarXz);

    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    let xopts = ExtractOptions {
        destination: dest.clone(),
        ..ExtractOptions::default()
    };
    archive.extract_all::<NullSink>(&xopts, None).expect("extract_all");
    assert_extracted(&dest, &members);
}

#[test]
fn tzst_alias_already_covered_by_f2_routes_to_tar_zst() {
    // F2 added `.tzst` -> TarZst; this is just a regression guard that
    // F4 didn't accidentally break the routing (e.g. by stealing the
    // extension into TarXz dispatch).
    let td = tempdir().unwrap();
    let src = td.path().join("bundle.tzst");
    let f = fs::File::create(&src).unwrap();
    let zenc = zstd::stream::write::Encoder::new(f, 3).unwrap().auto_finish();
    let members = members_fixture();
    let zenc = build_tar(zenc, &members);
    drop(zenc);

    let archive = Archive::open(&src, OpenMode::Read).expect("open .tzst");
    assert_eq!(archive.format(), ArchiveFormat::TarZst);
}

// --- dispatch sanity (non-tlz .txz still goes through LZMA2) --------------

#[test]
fn txz_dispatch_does_not_misfire_on_lzma1() {
    // Negative case: a .txz file should still be decoded as LZMA2.
    // Building one and feeding it to the LZMA1 decoder would EOF
    // immediately; this test verifies the dispatch picks the right
    // arm so we don't silently corrupt extracts.
    let td = tempdir().unwrap();
    let src = td.path().join("bundle.txz");
    let f = fs::File::create(&src).unwrap();
    let xenc = xz2::write::XzEncoder::new(f, 6);
    let members = members_fixture();
    let xenc = build_tar(xenc, &members);
    xenc.finish().unwrap();

    // The dispatch path matters more than the byte-equality here: we
    // only need extract_all to succeed without an LZMA stream error.
    let archive = Archive::open(&src, OpenMode::Read).expect("open .txz");
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    let opts = ExtractOptions {
        destination: dest.clone(),
        ..ExtractOptions::default()
    };
    archive
        .extract_all::<NullSink>(&opts, None)
        .expect(".txz must dispatch through LZMA2 decoder");
}
