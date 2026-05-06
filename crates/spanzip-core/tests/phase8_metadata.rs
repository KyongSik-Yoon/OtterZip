//! Phase 8 metadata-API tests. Covers the new accessors added to close
//! gap-report items G1 (`entry_count` / `is_encrypted` / `is_solid` /
//! `comment`) and G3 (FFI `spanzip_detect_format_bytes`).

use std::fs;
use std::io::Write;

use spanzip_core::{detect_bytes, Archive, ArchiveFormat, OpenMode};
use tempfile::tempdir;

fn build_zip(out: &std::path::Path, entries: &[(&str, &[u8])]) {
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

#[test]
fn entry_count_matches_actual_iteration() {
    let td = tempdir().unwrap();
    let p = td.path().join("a.zip");
    build_zip(&p, &[("a.txt", b"a"), ("b.txt", b"bb"), ("c.txt", b"ccc")]);

    let archive = Archive::open(&p, OpenMode::Read).unwrap();
    assert_eq!(archive.entry_count().unwrap(), 3);
}

#[test]
fn is_encrypted_false_for_plain_zip() {
    let td = tempdir().unwrap();
    let p = td.path().join("plain.zip");
    build_zip(&p, &[("a.txt", b"x")]);

    let archive = Archive::open(&p, OpenMode::Read).unwrap();
    assert!(!archive.is_encrypted().unwrap());
}

#[test]
fn is_encrypted_true_for_aes_zip() {
    let td = tempdir().unwrap();
    let p = td.path().join("crypt.zip");
    let f = fs::File::create(&p).unwrap();
    let mut w = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .with_aes_encryption(zip::AesMode::Aes256, "pw");
    w.start_file("secret.txt", opts).unwrap();
    w.write_all(b"top").unwrap();
    w.finish().unwrap();

    let archive = Archive::open(&p, OpenMode::Read).unwrap();
    assert!(archive.is_encrypted().unwrap());
}

#[test]
fn is_solid_returns_none_for_zip() {
    let td = tempdir().unwrap();
    let p = td.path().join("a.zip");
    build_zip(&p, &[("a.txt", b"x")]);
    let archive = Archive::open(&p, OpenMode::Read).unwrap();
    assert_eq!(archive.is_solid(), None);
}

#[test]
fn comment_is_none_when_unsupported() {
    let td = tempdir().unwrap();
    let p = td.path().join("a.zip");
    build_zip(&p, &[("a.txt", b"x")]);
    let archive = Archive::open(&p, OpenMode::Read).unwrap();
    assert!(archive.comment().is_none());
}

#[test]
fn detect_format_bytes_picks_zip_magic() {
    let zip_magic: &[u8] = &[0x50, 0x4B, 0x03, 0x04, 0xff, 0xff, 0xff];
    assert_eq!(detect_bytes(zip_magic), Some(ArchiveFormat::Zip));
}

#[test]
fn detect_format_bytes_picks_seven_zip_magic() {
    let sevenz_magic: &[u8] = &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00];
    assert_eq!(detect_bytes(sevenz_magic), Some(ArchiveFormat::SevenZ));
}

#[test]
fn detect_format_bytes_returns_none_on_garbage() {
    let nonsense: &[u8] = &[1, 2, 3, 4];
    assert_eq!(detect_bytes(nonsense), None);
}
