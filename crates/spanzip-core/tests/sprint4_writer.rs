//! Sprint 4 integration tests: archive *creation*. Round-trips a small
//! file tree through `Archive::create` + `add_file` + `commit`, then
//! re-opens via `Archive::open` and verifies content.

use std::fs;
use std::io::Write;

use spanzip_core::{
    Archive, ArchiveFormat, CreateOptions, ExtractOptions, OpenMode, OverwritePolicy,
};
use spanzip_core::format::CompressionMethod;
use tempfile::tempdir;

fn write_source_tree(root: &std::path::Path) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("a.txt"), b"alpha\n").unwrap();
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("nested/b.bin"), &[1u8, 2, 3, 4, 5]).unwrap();
}

fn extract_and_verify(archive_path: &std::path::Path, td: &std::path::Path) {
    let out = td.join("out");
    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let archive = Archive::open(archive_path, OpenMode::Read).unwrap();
    let report = archive
        .extract_all::<fn(&spanzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert!(report.entries_extracted >= 2);
    assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"alpha\n");
    assert_eq!(
        fs::read(out.join("nested").join("b.bin")).unwrap(),
        [1u8, 2, 3, 4, 5]
    );
}

#[test]
fn create_zip_then_extract_roundtrip() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let zip_path = td.path().join("out.zip");

    {
        let opts = CreateOptions {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            ..Default::default()
        };
        let mut archive = Archive::create(&zip_path, opts).unwrap();
        archive.add_file(src.join("a.txt"), "a.txt").unwrap();
        archive
            .add_file(src.join("nested/b.bin"), "nested/b.bin")
            .unwrap();
        archive.commit().unwrap();
    }

    extract_and_verify(&zip_path, td.path());
}

#[test]
fn create_zip_with_add_dir_recursive() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let zip_path = td.path().join("out.zip");

    {
        let opts = CreateOptions {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            ..Default::default()
        };
        let mut archive = Archive::create(&zip_path, opts).unwrap();
        archive
            .add_dir_recursive::<fn(&spanzip_core::Progress) -> bool>(&src, "", None)
            .unwrap();
        archive.commit().unwrap();
    }

    extract_and_verify(&zip_path, td.path());
}

#[test]
fn create_7z_then_extract_roundtrip() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let archive_path = td.path().join("out.7z");

    {
        let opts = CreateOptions {
            format: ArchiveFormat::SevenZ,
            compression: CompressionMethod::Lzma2,
            compression_level: 5,
            ..Default::default()
        };
        let mut archive = Archive::create(&archive_path, opts).unwrap();
        archive
            .add_dir_recursive::<fn(&spanzip_core::Progress) -> bool>(&src, "", None)
            .unwrap();
        archive.commit().unwrap();
    }

    extract_and_verify(&archive_path, td.path());
}

#[test]
fn create_targz_then_extract_roundtrip() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let archive_path = td.path().join("out.tar.gz");

    {
        let opts = CreateOptions {
            format: ArchiveFormat::TarGz,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            ..Default::default()
        };
        let mut archive = Archive::create(&archive_path, opts).unwrap();
        archive
            .add_dir_recursive::<fn(&spanzip_core::Progress) -> bool>(&src, "", None)
            .unwrap();
        archive.commit().unwrap();
    }

    extract_and_verify(&archive_path, td.path());
}

#[test]
fn rar_creation_is_explicitly_blocked() {
    let td = tempdir().unwrap();
    let opts = CreateOptions {
        format: ArchiveFormat::Rar,
        compression: CompressionMethod::Lzma2,
        compression_level: 5,
        ..Default::default()
    };
    let err = Archive::create(td.path().join("out.rar"), opts).unwrap_err();
    assert!(matches!(
        err,
        spanzip_core::SpanzipError::FeatureDisabled(_)
    ));
}

#[test]
fn rollback_removes_incomplete_archive() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let zip_path = td.path().join("incomplete.zip");

    let opts = CreateOptions {
        format: ArchiveFormat::Zip,
        compression: CompressionMethod::Deflate,
        compression_level: 5,
        ..Default::default()
    };
    let mut archive = Archive::create(&zip_path, opts).unwrap();
    archive.add_file(src.join("a.txt"), "a.txt").unwrap();
    archive.rollback().unwrap();

    assert!(!zip_path.exists(), "rollback should remove the file");
}

#[test]
fn add_file_on_read_mode_archive_errors() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let zip_path = td.path().join("seed.zip");
    {
        let opts = CreateOptions {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            ..Default::default()
        };
        let mut archive = Archive::create(&zip_path, opts).unwrap();
        archive.add_file(src.join("a.txt"), "a.txt").unwrap();
        archive.commit().unwrap();
    }

    let mut read = Archive::open(&zip_path, OpenMode::Read).unwrap();
    let err = read
        .add_file(src.join("a.txt"), "another.txt")
        .unwrap_err();
    assert!(matches!(
        err,
        spanzip_core::SpanzipError::InvalidArgument(_)
    ));
}
