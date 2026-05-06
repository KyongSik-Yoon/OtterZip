//! Sprint 1 integration test: create a ZIP with the `zip` crate, open it
//! via `spanzip_core::Archive`, enumerate entries, extract, verify bytes.
//!
//! Archive creation through our public API is Sprint 3 territory — this
//! test uses the `zip` crate directly as the fixture producer.

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

use spanzip_core::{Archive, ArchiveFormat, ExtractOptions, OpenMode, OverwritePolicy};
use tempfile::tempdir;
use zip::write::SimpleFileOptions;

const PAYLOAD_A: &[u8] = b"hello spanzip\n";
const PAYLOAD_B: &[u8] = b"second entry, slightly longer content for a sanity check\n";

fn build_fixture_zip(out: &std::path::Path) {
    let file = fs::File::create(out).expect("create fixture");
    let mut writer = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    writer.start_file("a.txt", opts).expect("start a.txt");
    writer.write_all(PAYLOAD_A).expect("write a.txt");

    writer.add_directory("sub/", opts).expect("add sub/");

    writer.start_file("sub/b.txt", opts).expect("start sub/b.txt");
    writer.write_all(PAYLOAD_B).expect("write sub/b.txt");

    writer.finish().expect("finish zip");
}

#[test]
fn format_detection_recognises_our_fixture() {
    let td = tempdir().unwrap();
    let zip_path = td.path().join("fixture.zip");
    build_fixture_zip(&zip_path);

    let fmt = spanzip_core::detect(&zip_path).unwrap();
    assert_eq!(fmt, ArchiveFormat::Zip);
}

#[test]
fn open_and_iterate_entries() {
    let td = tempdir().unwrap();
    let zip_path = td.path().join("fixture.zip");
    build_fixture_zip(&zip_path);

    let archive = Archive::open(&zip_path, OpenMode::Read).unwrap();
    assert_eq!(archive.format(), ArchiveFormat::Zip);
    assert_eq!(archive.path(), zip_path);

    let mut names = Vec::new();
    let mut sizes = Vec::new();
    for entry in archive.entries().unwrap() {
        let e = entry.unwrap();
        names.push(e.path.clone());
        sizes.push((e.path.clone(), e.uncompressed_size, e.is_directory));
    }

    assert!(names.iter().any(|n| n == "a.txt"));
    assert!(names.iter().any(|n| n == "sub/b.txt"));
    assert!(names.iter().any(|n| n == "sub/"));

    // Verify sizes pulled through correctly.
    let a = sizes.iter().find(|(n, _, _)| n == "a.txt").unwrap();
    assert_eq!(a.1, u64::try_from(PAYLOAD_A.len()).unwrap());
    assert!(!a.2);

    let b = sizes.iter().find(|(n, _, _)| n == "sub/b.txt").unwrap();
    assert_eq!(b.1, u64::try_from(PAYLOAD_B.len()).unwrap());
    assert!(!b.2);

    let d = sizes.iter().find(|(n, _, _)| n == "sub/").unwrap();
    assert!(d.2);
}

#[test]
fn extract_all_writes_files_with_correct_content() {
    let td = tempdir().unwrap();
    let zip_path = td.path().join("fixture.zip");
    build_fixture_zip(&zip_path);

    let out_dir: PathBuf = td.path().join("out");
    let opts = ExtractOptions {
        destination: out_dir.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };

    let archive = Archive::open(&zip_path, OpenMode::Read).unwrap();
    let report = archive.extract_all::<fn(&spanzip_core::Progress) -> bool>(&opts, None).unwrap();

    assert!(report.entries_extracted >= 2);
    assert_eq!(
        report.bytes_written,
        u64::try_from(PAYLOAD_A.len() + PAYLOAD_B.len()).unwrap()
    );

    // Verify file contents round-trip.
    let mut got = Vec::new();
    fs::File::open(out_dir.join("a.txt"))
        .unwrap()
        .read_to_end(&mut got)
        .unwrap();
    assert_eq!(got, PAYLOAD_A);

    got.clear();
    fs::File::open(out_dir.join("sub").join("b.txt"))
        .unwrap()
        .read_to_end(&mut got)
        .unwrap();
    assert_eq!(got, PAYLOAD_B);
}

#[test]
fn path_traversal_is_blocked_by_default() {
    // Craft a ZIP with an entry that tries to escape. We use `zip`'s writer
    // and stuff a nasty name into it via the raw writer.
    let td = tempdir().unwrap();
    let zip_path = td.path().join("evil.zip");

    {
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file("../escape.txt", opts).unwrap();
        writer.write_all(b"pwn").unwrap();
        writer.finish().unwrap();
    }

    let out_dir = td.path().join("out");
    let opts = ExtractOptions {
        destination: out_dir.clone(),
        overwrite: OverwritePolicy::Always,
        block_path_traversal: true,
        ..Default::default()
    };

    let archive = Archive::open(&zip_path, OpenMode::Read).unwrap();
    let err = archive
        .extract_all::<fn(&spanzip_core::Progress) -> bool>(&opts, None)
        .unwrap_err();
    match err {
        spanzip_core::SpanzipError::PathTraversalBlocked(_) => {}
        other => panic!("expected PathTraversalBlocked, got {other:?}"),
    }

    // Ensure nothing leaked outside `out_dir`.
    assert!(!td.path().join("escape.txt").exists());
}

#[test]
fn truly_unsupported_format_rejected() {
    // Phase 7+ option Y (PR-F1) made bare GZIP / BZIP2 / XZ / LZMA into
    // first-class single-stream archive backends, closing the schema §5.2
    // gap. The original `unsupported_formats_rejected` test treated a
    // lone .gz as the canonical "rejected" case — which no longer holds.
    //
    // The contract we still need to verify is: a file that detect() can't
    // classify (no magic match, no extension hint) must yield
    // `UnsupportedFormat`. Use random non-magic bytes to exercise that.
    let td = tempdir().unwrap();
    let bogus = td.path().join("payload.bogusext");
    fs::write(&bogus, b"\x00\x01\x02\x03not-a-real-archive\x00").unwrap();
    let err = Archive::open(&bogus, OpenMode::Read).unwrap_err();
    match err {
        spanzip_core::SpanzipError::UnsupportedFormat(_) => {}
        other => panic!("expected UnsupportedFormat for bogus extension, got {other:?}"),
    }
}
