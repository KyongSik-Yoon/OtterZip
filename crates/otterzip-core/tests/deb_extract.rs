//! Phase 7+ option Y — PR-F8 acceptance tests for Debian package
//! extraction.
//!
//! Coverage matrix per `docs/05-build/phase-7-plus-plan.md` PR-F8:
//! 1. Round-trip — author a minimal DEB (3 ar members: `debian-
//!    binary`, `control.tar.gz`, `data.tar.gz`) and verify each
//!    member round-trips byte-equal through Archive::open +
//!    extract_all.
//! 2. The data tarball is decoded with the inner gzip+tar path the
//!    user would take after extracting -- this confirms the DEB
//!    backend hands back valid tar.gz bytes (not a chopped stream).
//! 3. Detection — `.deb` extension and the `!<arch>\n` magic both
//!    route to ArchiveFormat::Deb; non-.deb ar files fall back to
//!    Unknown so static libraries (.a / .ar) don't get extracted.
//! 4. Negative — missing magic / truncated header surface clean
//!    errors, not panics.
//! 5. Creation modes are rejected (extract-only).

use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

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
// Minimal ar / DEB builder
// =========================================================================
// 60-byte fixed-width headers per BSD ar(5). We hand-roll the
// builder rather than depending on an external `ar` crate -- the
// format is small, the test is self-contained, and the parser we're
// exercising in `backends/deb.rs` is hand-rolled too, so the
// fixture / backend pair share the same understanding of the format.

fn write_ar_member<W: Write>(w: &mut W, name: &str, content: &[u8]) {
    // Name field: 16 bytes, terminated by '/', space-padded.
    let mut name_field = [b' '; 16];
    let display = format!("{}/", name);
    let bytes = display.as_bytes();
    let n = bytes.len().min(16);
    name_field[..n].copy_from_slice(&bytes[..n]);

    // mtime / uid / gid / mode -- all zero so the test is
    // byte-deterministic across platforms.
    let mut header = Vec::with_capacity(60);
    header.extend_from_slice(&name_field);
    header.extend_from_slice(b"0           "); // mtime  (12)
    header.extend_from_slice(b"0     "); // uid    (6)
    header.extend_from_slice(b"0     "); // gid    (6)
    header.extend_from_slice(b"0       "); // mode   (8)

    // Size field: 10 bytes, ASCII decimal, space-padded.
    let mut size_field = [b' '; 10];
    let s = content.len().to_string();
    let sb = s.as_bytes();
    size_field[..sb.len()].copy_from_slice(sb);
    header.extend_from_slice(&size_field);

    // End marker
    header.extend_from_slice(b"`\n");
    assert_eq!(header.len(), 60);

    w.write_all(&header).unwrap();
    w.write_all(content).unwrap();
    if content.len() % 2 == 1 {
        w.write_all(b"\n").unwrap();
    }
}

fn make_data_tar_gz(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let genc = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
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
    buf.into_inner()
}

fn write_deb(path: &Path, control_tar_gz: &[u8], data_tar_gz: &[u8]) {
    let mut f = fs::File::create(path).unwrap();
    f.write_all(b"!<arch>\n").unwrap();
    write_ar_member(&mut f, "debian-binary", b"2.0\n");
    write_ar_member(&mut f, "control.tar.gz", control_tar_gz);
    write_ar_member(&mut f, "data.tar.gz", data_tar_gz);
}

// =========================================================================
// Tests
// =========================================================================

#[test]
fn deb_round_trip_three_members() {
    let td = tempdir().unwrap();
    let src = td.path().join("fixture.deb");

    // Minimal control.tar.gz (single Control file is enough).
    let control_tar_gz = make_data_tar_gz(&[("control", b"Package: otterzip-test\n")]);
    let data_payload: &[u8] = b"hello from inside the data tarball\n";
    let data_tar_gz = make_data_tar_gz(&[("usr/bin/hello", data_payload)]);

    write_deb(&src, &control_tar_gz, &data_tar_gz);

    let archive = Archive::open(&src, OpenMode::Read).expect("open .deb");
    assert_eq!(archive.format(), ArchiveFormat::Deb);

    let entries: Vec<_> = archive
        .entries()
        .expect("entries")
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 3, "DEB must surface 3 ar members");
    let names: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    assert!(names.contains(&"debian-binary"));
    assert!(names.contains(&"control.tar.gz"));
    assert!(names.contains(&"data.tar.gz"));

    let dest = td.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    let opts = ExtractOptions {
        destination: dest.clone(),
        ..ExtractOptions::default()
    };
    archive
        .extract_all::<NullSink>(&opts, None)
        .expect("extract_all");

    // 1. debian-binary content matches verbatim.
    let bin = fs::read(dest.join("debian-binary")).expect("debian-binary missing");
    assert_eq!(bin, b"2.0\n");

    // 2. The extracted data.tar.gz must round-trip back through the
    //    standard tar.gz path -- this is the "tar wrapper integrity"
    //    check the plan calls for. We re-open the extracted file
    //    through Archive again and verify the inner payload comes
    //    out byte-equal.
    let inner_path = dest.join("data.tar.gz");
    let inner_archive = Archive::open(&inner_path, OpenMode::Read).expect("re-open data.tar.gz");
    assert_eq!(inner_archive.format(), ArchiveFormat::TarGz);
    let inner_dest = td.path().join("data-out");
    fs::create_dir_all(&inner_dest).unwrap();
    let inner_opts = ExtractOptions {
        destination: inner_dest.clone(),
        ..ExtractOptions::default()
    };
    inner_archive
        .extract_all::<NullSink>(&inner_opts, None)
        .expect("inner extract");
    let payload = fs::read(inner_dest.join("usr/bin/hello")).expect("payload missing");
    assert_eq!(payload, data_payload, "inner data.tar.gz payload mismatch");
}

#[test]
fn deb_extension_routes_to_deb_variant() {
    let td = tempdir().unwrap();
    let src = td.path().join("empty.deb");
    fs::write(&src, b"").unwrap();
    assert_eq!(otterzip_core::detect(&src).unwrap(), ArchiveFormat::Deb);
}

#[test]
fn ar_magic_routes_to_deb_only_when_extension_agrees() {
    let td = tempdir().unwrap();

    // Real .deb extension + magic -> Deb.
    let deb = td.path().join("real.deb");
    fs::write(&deb, b"!<arch>\nrest").unwrap();
    assert_eq!(otterzip_core::detect(&deb).unwrap(), ArchiveFormat::Deb);

    // Static library (.a) with the same magic -> Unknown by policy
    // (we don't extract object archives).
    let staticlib = td.path().join("libfoo.a");
    fs::write(&staticlib, b"!<arch>\nrest").unwrap();
    assert_eq!(otterzip_core::detect(&staticlib).unwrap(), ArchiveFormat::Unknown);
}

#[test]
fn truncated_deb_header_yields_clean_error() {
    let td = tempdir().unwrap();
    let src = td.path().join("truncated.deb");
    // Magic + start of a 60-byte header but only 10 bytes -- the
    // backend's read_exact should fail with a regular IO error
    // rather than a panic.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"!<arch>\n");
    bytes.extend_from_slice(b"abcdefghij"); // 10 of 60
    fs::write(&src, &bytes).unwrap();

    let result = Archive::open(&src, OpenMode::Read);
    assert!(
        result.is_err(),
        "truncated DEB must surface an error rather than succeed"
    );
}

#[test]
fn missing_magic_yields_unsupported() {
    let td = tempdir().unwrap();
    let src = td.path().join("garbage.deb");
    fs::write(&src, b"definitely not an ar archive at all").unwrap();
    let result = Archive::open(&src, OpenMode::Read);
    match result {
        Err(OtterzipError::UnsupportedFormat(_)) | Err(OtterzipError::BackendError(_)) => {}
        other => panic!("expected UnsupportedFormat / BackendError, got {other:?}"),
    }
}

#[test]
fn deb_creation_modes_rejected() {
    let td = tempdir().unwrap();
    let src = td.path().join("seed.deb");
    let ctrl = make_data_tar_gz(&[("control", b"Package: t\n")]);
    let data = make_data_tar_gz(&[("hello", b"x")]);
    write_deb(&src, &ctrl, &data);

    for mode in [OpenMode::Update, OpenMode::CreateNew, OpenMode::CreateOrOverwrite] {
        let result = Archive::open(&src, mode);
        assert!(result.is_err(), "DEB in {mode:?} mode must be rejected");
    }
}
