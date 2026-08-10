//! Phase 7+ option Y — PR-F6 acceptance tests for the Windows
//! installer family (CAB + MSI).
//!
//! Coverage matrix per `docs/05-build/phase-7-plus-plan.md` PR-F6:
//! 1. CAB round-trip — author a tiny multi-file Cabinet via the
//!    `cab` 0.6 builder, open it through `Archive`, extract, and
//!    compare payload bytes.
//! 2. CAB detection routes `.cab` extensions and the `MSCF` magic
//!    to `ArchiveFormat::Cab`.
//! 3. MSI round-trip — author a tiny CFB compound document via the
//!    `cfb` 0.10 writer, populate it with one storage + two
//!    streams, open through `Archive`, verify both streams come
//!    out byte-equal.
//! 4. MSI detection — `.msi` + the OLE2 signature both route to
//!    `ArchiveFormat::Msi`.
//! 5. Negative cases — corrupted CAB / random `.msi` data yield a
//!    clean error rather than a panic.
//! 6. Creation modes are rejected for both formats (extract-only
//!    by policy).

use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

use cab::{CabinetBuilder, CompressionType};
use cfb::CompoundFile;
use otterzip_core::{
    Archive, ArchiveFormat, ExtractOptions, OpenMode, ProgressSink, OtterzipError,
};
use tempfile::tempdir;

struct NullSink;
impl ProgressSink for NullSink {
    fn update(&mut self, _: &otterzip_core::Progress) -> bool {
        true
    }
}

// =========================================================================
// CAB fixture
// =========================================================================

fn write_cab(path: &Path, files: &[(&str, &[u8])]) {
    let f = fs::File::create(path).unwrap();
    let mut builder = CabinetBuilder::new();
    let folder = builder.add_folder(CompressionType::None);
    for (name, _) in files {
        folder.add_file(*name);
    }
    let mut writer = builder.build(f).expect("cab build");
    let mut idx = 0usize;
    while let Some(mut fw) = writer.next_file().expect("next_file") {
        fw.write_all(files[idx].1).expect("write file body");
        idx += 1;
    }
    writer.finish().expect("cab finish");
    assert_eq!(idx, files.len(), "wrote {idx} files, expected {}", files.len());
}

fn extract_cab(src: &Path, dest: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let archive = Archive::open(src, OpenMode::Read).expect("open cab");
    assert_eq!(archive.format(), ArchiveFormat::Cab);
    let opts = ExtractOptions {
        destination: dest.to_path_buf(),
        ..ExtractOptions::default()
    };
    archive
        .extract_all::<NullSink>(&opts, None)
        .expect("extract_all");
    let mut out = Vec::new();
    for de in walkdir(dest) {
        if de.is_file() {
            out.push((de.clone(), fs::read(&de).unwrap()));
        }
    }
    out
}

fn walkdir(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(root) {
        for de in rd.flatten() {
            let p = de.path();
            if p.is_dir() {
                out.extend(walkdir(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[test]
fn cab_basic_round_trip() {
    let payload_a = b"alpha file inside cabinet\n" as &[u8];
    let payload_b = b"beta-with-binary\x00\x01\x02\x03" as &[u8];
    let td = tempdir().unwrap();
    let src = td.path().join("fixture.cab");
    write_cab(&src, &[("ALPHA.TXT", payload_a), ("BETA.BIN", payload_b)]);

    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    let extracted = extract_cab(&src, &dest);

    let mut found_a = false;
    let mut found_b = false;
    for (_p, bytes) in &extracted {
        if bytes == payload_a {
            found_a = true;
        } else if bytes == payload_b {
            found_b = true;
        }
    }
    assert!(found_a, "ALPHA.TXT payload not extracted");
    assert!(found_b, "BETA.BIN payload not extracted");
}

#[test]
fn cab_magic_routes_to_cab_variant() {
    // Extension is bogus -- the MSCF magic alone should classify
    // the file. This guards against a future change that strips
    // the magic-byte path in detect_bytes.
    let td = tempdir().unwrap();
    let src = td.path().join("payload.unknown");
    write_cab(&src, &[("X.TXT", b"x")]);
    assert_eq!(otterzip_core::detect(&src).unwrap(), ArchiveFormat::Cab);
}

#[test]
fn cab_extension_routes_to_cab_variant() {
    let td = tempdir().unwrap();
    let src = td.path().join("empty.cab");
    fs::write(&src, b"").unwrap();
    assert_eq!(otterzip_core::detect(&src).unwrap(), ArchiveFormat::Cab);
}

#[test]
fn random_bytes_in_cab_extension_yield_unsupported() {
    let td = tempdir().unwrap();
    let src = td.path().join("garbage.cab");
    fs::write(&src, b"not a real cabinet at all").unwrap();
    let result = Archive::open(&src, OpenMode::Read);
    match result {
        Err(OtterzipError::UnsupportedFormat(_)) | Err(OtterzipError::BackendError(_)) => {}
        other => panic!("expected UnsupportedFormat / BackendError, got {other:?}"),
    }
}

// =========================================================================
// MSI fixture
// =========================================================================

// CFB paths are addressed with `/`, never `\`. The `cfb` crate routes them
// through `std::path::Path::components`, which is host-dependent: on Windows
// a `\Inner\StreamTwo` splits into two components, on Unix it stays ONE
// component whose name contains backslashes — and `validate_name` rejects
// those, tripping a debug assertion inside the crate. `/` parses identically
// on both hosts, so the fixture uses it.
fn write_msi(path: &Path, streams: &[(&str, &[u8])]) {
    let f = fs::File::create(path).unwrap();
    let mut comp = CompoundFile::create(f).expect("cfb create");
    for (cfb_path, body) in streams {
        // Auto-create any parent storages so a path like
        // `/Binary/Foo` works without a separate setup step.
        if let Some(parent_idx) = cfb_path.rfind('/') {
            if parent_idx > 0 {
                let parent = &cfb_path[..parent_idx];
                let _ = comp.create_storage_all(parent);
            }
        }
        let mut s = comp.create_stream(*cfb_path).expect("create_stream");
        s.write_all(body).expect("write stream");
    }
    comp.flush().expect("cfb flush");
}

#[test]
fn msi_basic_round_trip() {
    let payload_a = b"first stream contents" as &[u8];
    let payload_b = b"second stream\x00\x01" as &[u8];
    let td = tempdir().unwrap();
    let src = td.path().join("fixture.msi");
    write_msi(
        &src,
        &[
            ("/StreamOne", payload_a),
            ("/Inner/StreamTwo", payload_b),
        ],
    );

    let archive = Archive::open(&src, OpenMode::Read).expect("open msi");
    assert_eq!(archive.format(), ArchiveFormat::Msi);

    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    let opts = ExtractOptions {
        destination: dest.clone(),
        ..ExtractOptions::default()
    };
    archive
        .extract_all::<NullSink>(&opts, None)
        .expect("extract_all");

    // Find both payloads anywhere in the dest tree.
    let mut found_a = false;
    let mut found_b = false;
    for p in walkdir(&dest) {
        if !p.is_file() {
            continue;
        }
        let bytes = fs::read(&p).unwrap();
        if bytes == payload_a {
            found_a = true;
        } else if bytes == payload_b {
            found_b = true;
        }
    }
    assert!(found_a, "MSI StreamOne payload not extracted");
    assert!(found_b, "MSI Inner/StreamTwo payload not extracted");
}

#[test]
fn msi_extension_routes_to_msi_variant() {
    let td = tempdir().unwrap();
    let src = td.path().join("empty.msi");
    // Empty file -- the magic check will fail, but the extension
    // hint should still classify it as Msi at the dispatcher level.
    fs::write(&src, b"").unwrap();
    assert_eq!(otterzip_core::detect(&src).unwrap(), ArchiveFormat::Msi);
}

#[test]
fn ole2_magic_with_non_msi_extension_falls_back_to_unknown() {
    // OLE2 signature is shared with .doc / .xls / .ppt. Per
    // upgrade_with_extension, those should fall back to Unknown
    // so the user gets a "we don't extract Word documents" signal
    // rather than a confusing "MSI is corrupt" error.
    let td = tempdir().unwrap();
    let src = td.path().join("document.doc");
    let mut buf = vec![0u8; 4096];
    buf[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    fs::write(&src, &buf).unwrap();
    assert_eq!(otterzip_core::detect(&src).unwrap(), ArchiveFormat::Unknown);
}

#[test]
fn random_bytes_in_msi_extension_yield_unsupported() {
    let td = tempdir().unwrap();
    let src = td.path().join("garbage.msi");
    fs::write(&src, b"definitely not a compound document").unwrap();
    let result = Archive::open(&src, OpenMode::Read);
    match result {
        Err(OtterzipError::UnsupportedFormat(_)) | Err(OtterzipError::BackendError(_)) => {}
        other => panic!("expected UnsupportedFormat / BackendError, got {other:?}"),
    }
}

// =========================================================================
// Creation policy
// =========================================================================

#[test]
fn cab_and_msi_creation_modes_rejected() {
    let td = tempdir().unwrap();
    let cab_src = td.path().join("seed.cab");
    write_cab(&cab_src, &[("X.TXT", b"x")]);
    let msi_src = td.path().join("seed.msi");
    write_msi(&msi_src, &[("/X", b"x")]);

    for src in [&cab_src, &msi_src] {
        for mode in [
            OpenMode::Update,
            OpenMode::CreateNew,
            OpenMode::CreateOrOverwrite,
        ] {
            let result = Archive::open(src, mode);
            assert!(
                result.is_err(),
                "{:?} in {:?} mode must be rejected (extract-only)",
                src.file_name(),
                mode
            );
        }
    }
}

// Silence unused -- io::Cursor was needed when an earlier draft used
// in-memory cabinets. Keep the import handy for future fixture work.
#[allow(dead_code)]
fn _force_use(_: Cursor<Vec<u8>>) {}
