//! Phase 7+ option Y — PR-F3 acceptance tests for ZIP variant aliases.
//!
//! Coverage matrix per `docs/05-build/phase-7-plus-plan.md` PR-F3:
//! 1. `extension_hint` routes every alias to `ArchiveFormat::Zip`
//!    (covered as unit tests in `format.rs`).
//! 2. The full `Archive::open` -> `extract_all` pipeline works
//!    end-to-end on a fixture saved with a non-`.zip` extension.
//! 3. The fixture used here has a real ZIP magic header so the
//!    detect path takes the magic-bytes branch -- the alias hint is
//!    only a fallback for empty / signature-less files. We still
//!    verify both paths converge on the same backend.
//!
//! These aliases (JAR / WAR / EAR / IPA / APK / AAB / APPX / MSIX /
//! XPI / CRX) are all ZIP containers with extra structural conventions
//! on top, so re-running every ZIP regression test for each one would
//! be wasteful. Instead we pick three representatives -- JAR (Java
//! ecosystem), APK (Android), MSIX (Windows store) -- and trust that
//! the shared `ZipBackend` handles the rest identically.

use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

use otterzip_core::{Archive, ArchiveFormat, ExtractOptions, OpenMode, ProgressSink};
use tempfile::tempdir;

struct NullSink;
impl ProgressSink for NullSink {
    fn update(&mut self, _: &otterzip_core::Progress) -> bool {
        true
    }
}

/// Build a tiny in-memory ZIP archive containing the given members and
/// dump it to `path`. No compression, no encryption -- the archive is a
/// transport-only fixture; we just want a valid PK\x03\x04 stream.
fn write_zip_with_members(path: &Path, members: &[(&str, &[u8])]) {
    let cursor = Cursor::new(Vec::<u8>::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in members {
        writer.start_file::<_, ()>(*name, opts).expect("start_file");
        writer.write_all(data).expect("write");
    }
    let cursor = writer.finish().expect("finish");
    let bytes = cursor.into_inner();
    fs::write(path, bytes).expect("write fixture");
}

fn extract_all_to(archive_path: &Path, dest: &Path) {
    let archive = Archive::open(archive_path, OpenMode::Read).expect("open");
    let opts = ExtractOptions {
        destination: dest.to_path_buf(),
        ..ExtractOptions::default()
    };
    archive
        .extract_all::<NullSink>(&opts, None)
        .expect("extract_all");
}

fn assert_member_extracted(dest: &Path, name: &str, expected: &[u8]) {
    let p = dest.join(name);
    let actual = fs::read(&p).unwrap_or_else(|_| panic!("missing {} at {:?}", name, p));
    assert_eq!(actual, expected, "member {} byte mismatch", name);
}

// --- representative aliases (JAR / APK / MSIX) ----------------------------

#[test]
fn jar_alias_extracts_through_zip_backend() {
    let td = tempdir().unwrap();
    let src = td.path().join("library.jar");
    let members: &[(&str, &[u8])] = &[
        ("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n"),
        ("com/example/Hello.class", &[0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 0]),
    ];
    write_zip_with_members(&src, members);

    let archive = Archive::open(&src, OpenMode::Read).expect("open .jar");
    assert_eq!(
        archive.format(),
        ArchiveFormat::Zip,
        "JAR alias must dispatch to the ZIP backend"
    );
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    extract_all_to(&src, &dest);
    for (name, data) in members {
        assert_member_extracted(&dest, name, data);
    }
}

#[test]
fn apk_alias_extracts_through_zip_backend() {
    let td = tempdir().unwrap();
    let src = td.path().join("MyApp.apk");
    let members: &[(&str, &[u8])] = &[
        ("AndroidManifest.xml", b"<manifest/>"),
        ("classes.dex", &[0u8; 64]),
        ("res/values/strings.xml", b"<resources/>"),
    ];
    write_zip_with_members(&src, members);

    let archive = Archive::open(&src, OpenMode::Read).expect("open .apk");
    assert_eq!(archive.format(), ArchiveFormat::Zip);
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    extract_all_to(&src, &dest);
    for (name, data) in members {
        assert_member_extracted(&dest, name, data);
    }
}

#[test]
fn msix_alias_extracts_through_zip_backend() {
    let td = tempdir().unwrap();
    let src = td.path().join("Sample.msix");
    let members: &[(&str, &[u8])] = &[
        ("AppxManifest.xml", b"<Package/>"),
        ("[Content_Types].xml", b"<Types/>"),
        ("Assets/Logo.png", &[0u8, 1, 2, 3, 4, 5, 6, 7]),
    ];
    write_zip_with_members(&src, members);

    let archive = Archive::open(&src, OpenMode::Read).expect("open .msix");
    assert_eq!(archive.format(), ArchiveFormat::Zip);
    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    extract_all_to(&src, &dest);
    for (name, data) in members {
        assert_member_extracted(&dest, name, data);
    }
}

// --- detection fallback (no magic, alias extension only) ------------------

#[test]
fn alias_extension_hint_fires_when_magic_absent() {
    // An empty file has no magic bytes -- detect must fall back to the
    // extension hint. This guarantees PR-F3's contract: even pathological
    // inputs without a valid PK header get classified as ZIP when the
    // extension is one of our aliases. The eventual `Archive::open` will
    // fail with a backend error (corrupted archive), but at the dispatch
    // layer we still want the right routing.
    let td = tempdir().unwrap();
    let src = td.path().join("empty.xpi");
    fs::write(&src, b"").unwrap();
    assert_eq!(otterzip_core::detect(&src).unwrap(), ArchiveFormat::Zip);
}
