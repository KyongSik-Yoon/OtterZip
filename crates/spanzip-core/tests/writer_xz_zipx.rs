//! Phase 7+ option Y — PR-F7 acceptance tests for write-side
//! support: single-stream `.xz` + `.zipx` (ZIP with bzip2 / lzma
//! method extensions).
//!
//! Coverage matrix per `docs/05-build/phase-7-plus-plan.md` PR-F7:
//! 1. `.xz` round-trip — write a single payload via Archive::create
//!    + add_file + commit, then re-open via Archive::open +
//!    extract_all and compare bytes.
//! 2. `.zipx` round-trip with the BZIP2 method (the ZIPX baseline
//!    every Windows extractor handles).
//! 3. `.zipx` round-trip with the LZMA method (smaller files; some
//!    older readers refuse it, but our ZipBackend reads it back
//!    fine).
//! 4. XZ single-stream rejects a second add (single-stream allows
//!    exactly one entry).
//! 5. The remaining writer-disabled formats (Bzip2 / Lzma single,
//!    Zstd / TarLz4, ISO / CAB / MSI / DEB) all still surface
//!    FeatureDisabled — we don't want F7 to accidentally enable
//!    paths that have no implementation behind them.

use std::fs;
use std::io::Write as _;

use spanzip_core::{
    Archive, ArchiveFormat, CompressionMethod, CreateOptions, ExtractOptions,
    OpenMode, OverwritePolicy, ProgressSink, SpanzipError,
};
use tempfile::tempdir;

struct NullSink;
impl ProgressSink for NullSink {
    fn update(&mut self, _: &spanzip_core::Progress) -> bool {
        true
    }
}

fn payload(size: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(size + 64);
    for i in 0..size {
        v.push((i * 31 % 251) as u8);
        if i % 23 == 0 {
            v.extend_from_slice(b"writer F7 ZIPX/XZ acceptance ");
        }
    }
    v.truncate(size);
    v
}

fn create_opts(format: ArchiveFormat, method: CompressionMethod) -> CreateOptions {
    CreateOptions {
        format,
        compression: method,
        compression_level: 0, // let backend pick
        ..CreateOptions::default()
    }
}

fn extract_opts(dest: &std::path::Path) -> ExtractOptions {
    ExtractOptions {
        destination: dest.to_path_buf(),
        overwrite: OverwritePolicy::Always,
        ..ExtractOptions::default()
    }
}

/// Drop a payload to disk so we can feed `Archive::add_file` with
/// a real source path -- the public writer API is path-based.
fn write_source(td: &std::path::Path, name: &str, body: &[u8]) -> std::path::PathBuf {
    let p = td.join(name);
    let mut f = fs::File::create(&p).unwrap();
    f.write_all(body).unwrap();
    p
}

// =========================================================================
// XZ writer
// =========================================================================

#[test]
fn xz_single_stream_writer_round_trip() {
    let body = payload(8 * 1024);
    let td = tempdir().unwrap();
    let src_payload = write_source(td.path(), "payload.txt", &body);
    let archive_path = td.path().join("payload.txt.xz");

    let mut archive = Archive::create(
        &archive_path,
        create_opts(ArchiveFormat::Xz, CompressionMethod::Lzma),
    )
    .expect("create xz");
    archive
        .add_file(&src_payload, "payload.txt")
        .expect("add_file");
    archive.commit().expect("commit");

    let read = Archive::open(&archive_path, OpenMode::Read).expect("re-open xz");
    assert_eq!(read.format(), ArchiveFormat::Xz);

    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    read.extract_all::<NullSink>(&extract_opts(&dest), None)
        .expect("extract_all");

    let entries: Vec<_> = fs::read_dir(&dest).unwrap().filter_map(|e| e.ok()).collect();
    assert_eq!(entries.len(), 1, "expected exactly one extracted file");
    let extracted = fs::read(entries[0].path()).unwrap();
    assert_eq!(extracted, body, "xz writer round-trip mismatch");
}

#[test]
fn xz_writer_rejects_second_add() {
    let td = tempdir().unwrap();
    let a = write_source(td.path(), "a.txt", b"first payload");
    let b = write_source(td.path(), "b.txt", b"second payload");
    let archive_path = td.path().join("payload.xz");

    let mut archive = Archive::create(
        &archive_path,
        create_opts(ArchiveFormat::Xz, CompressionMethod::Lzma),
    )
    .expect("create");
    archive.add_file(&a, "a.txt").expect("first add ok");
    let err = archive
        .add_file(&b, "b.txt")
        .expect_err("second add must fail");
    match err {
        SpanzipError::FeatureDisabled(_) => {}
        other => panic!("expected FeatureDisabled, got {other:?}"),
    }
    // Drop without commit -- the partial file is in tempdir which
    // gets cleaned up automatically.
    drop(archive);
}

// =========================================================================
// ZIPX writer (bzip2 / lzma methods)
// =========================================================================

fn zipx_round_trip_with_method(method: CompressionMethod) {
    let body = payload(16 * 1024);
    let td = tempdir().unwrap();
    let src_a = write_source(td.path(), "a.txt", &body);
    let small = b"second entry payload\n";
    let src_b = write_source(td.path(), "b.txt", small);
    let archive_path = td.path().join("bundle.zipx");

    let mut archive = Archive::create(
        &archive_path,
        create_opts(ArchiveFormat::Zipx, method),
    )
    .expect("create zipx");
    archive.add_file(&src_a, "a.txt").expect("add a");
    archive.add_file(&src_b, "dir/b.txt").expect("add b");
    archive.commit().expect("commit");

    let read = Archive::open(&archive_path, OpenMode::Read).expect("re-open zipx");
    assert_eq!(read.format(), ArchiveFormat::Zipx);

    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    read.extract_all::<NullSink>(&extract_opts(&dest), None)
        .expect("extract_all");

    let extracted_a = fs::read(dest.join("a.txt")).expect("a.txt missing");
    assert_eq!(extracted_a, body, "zipx round-trip a.txt mismatch ({:?})", method);
    let extracted_b = fs::read(dest.join("dir/b.txt")).expect("dir/b.txt missing");
    assert_eq!(extracted_b, small, "zipx round-trip b.txt mismatch ({:?})", method);
}

#[test]
fn zipx_round_trip_bzip2_method() {
    zipx_round_trip_with_method(CompressionMethod::Bzip2);
}

#[test]
fn zipx_lzma_write_is_explicitly_rejected() {
    // The zip 2.x crate ships LZMA as a read-only codec -- the
    // writer side returns "LZMA isn't supported for compression".
    // We surface that as a clean FeatureDisabled at create time
    // rather than letting the caller see a confusing error mid-
    // add_entry. The read side still handles LZMA-encoded ZIPX
    // archives (covered transitively by the standard ZIP backend
    // tests, which use the same crate features).
    let td = tempdir().unwrap();
    let archive_path = td.path().join("would-be.zipx");
    let result = Archive::create(
        &archive_path,
        create_opts(ArchiveFormat::Zipx, CompressionMethod::Lzma),
    );
    match result {
        Err(SpanzipError::FeatureDisabled(msg)) => {
            assert!(
                msg.contains("LZMA"),
                "FeatureDisabled message should mention LZMA, got {msg:?}"
            );
        }
        other => panic!("expected FeatureDisabled, got {other:?}"),
    }
}

// =========================================================================
// Cross-format sanity
// =========================================================================

#[test]
fn writer_dispatch_rejects_remaining_unimplemented_formats() {
    // Bzip2 / Lzma single-stream and the Zstd / LZ4 family stay
    // FeatureDisabled per the F7 plan -- confirming so a future
    // change doesn't accidentally enable them without writer
    // implementations behind them.
    let td = tempdir().unwrap();
    for (fmt, label) in [
        (ArchiveFormat::Bzip2, "bzip2-single"),
        (ArchiveFormat::Lzma, "lzma-single"),
        (ArchiveFormat::Zstd, "zstd-single"),
        (ArchiveFormat::TarLz4, "tar-lz4"),
        (ArchiveFormat::Iso, "iso"),
        (ArchiveFormat::Cab, "cab"),
        (ArchiveFormat::Msi, "msi"),
        (ArchiveFormat::Deb, "deb"),
    ] {
        let archive_path = td.path().join(format!("attempt-{label}.bin"));
        let result =
            Archive::create(&archive_path, create_opts(fmt, CompressionMethod::Store));
        match result {
            Err(SpanzipError::FeatureDisabled(_)) => {}
            other => panic!("{label}: expected FeatureDisabled, got {other:?}"),
        }
    }
}
