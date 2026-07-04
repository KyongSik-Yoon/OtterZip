//! Phase 7 — security hardening regression tests.
//!
//! Each scenario crafts a small ZIP with one or more "weaponised"
//! entries and asserts that `Archive::extract_all` rejects them with
//! the expected error class. These failures must stay loud — silent
//! coercion of e.g. `CON.txt` to `CON` would re-open known Windows
//! shell-parsing CVEs.

use std::fs;
use std::io::Write;

use otterzip_core::{
    Archive, ExtractOptions, OpenMode, OverwritePolicy, Progress, OtterzipError,
};
use tempfile::tempdir;
use zip::write::SimpleFileOptions;

fn write_zip_with(name: &str, body: &[u8], path: &std::path::Path) {
    let file = fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file(name, opts).unwrap();
    writer.write_all(body).unwrap();
    writer.finish().unwrap();
}

fn extract_with_defaults(zip: &std::path::Path, dest: &std::path::Path) -> otterzip_core::Result<()> {
    let opts = ExtractOptions {
        destination: dest.to_path_buf(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let archive = Archive::open(zip, OpenMode::Read)?;
    archive
        .extract_all::<fn(&Progress) -> bool>(&opts, None)
        .map(|_| ())
}

// ---------------------------------------------------------------------------
// Path-segment hardening
// ---------------------------------------------------------------------------

#[test]
fn rejects_ntfs_alternate_data_stream() {
    let td = tempdir().unwrap();
    let zip = td.path().join("ads.zip");
    write_zip_with("notes.txt:hidden", b"pwn", &zip);

    let err = extract_with_defaults(&zip, &td.path().join("out")).unwrap_err();
    assert!(
        matches!(err, OtterzipError::PathTraversalBlocked(_)),
        "ADS must be blocked, got {err:?}"
    );
}

#[test]
fn rejects_windows_reserved_names() {
    for name in &["CON.txt", "PRN.dat", "AUX.bin", "NUL.log", "COM1.cfg", "LPT1.tmp"] {
        let td = tempdir().unwrap();
        let zip = td.path().join("reserved.zip");
        write_zip_with(name, b"x", &zip);
        let err = extract_with_defaults(&zip, &td.path().join("out")).unwrap_err();
        assert!(
            matches!(err, OtterzipError::PathTraversalBlocked(_)),
            "reserved name {name} must be blocked, got {err:?}"
        );
    }
}

#[test]
fn rejects_embedded_backslash() {
    let td = tempdir().unwrap();
    let zip = td.path().join("backslash.zip");
    write_zip_with("a\\..\\..\\evil.txt", b"x", &zip);

    let err = extract_with_defaults(&zip, &td.path().join("out")).unwrap_err();
    assert!(
        matches!(err, OtterzipError::PathTraversalBlocked(_)),
        "embedded backslash must be blocked, got {err:?}"
    );
}

#[test]
fn rejects_trailing_dot_or_space() {
    for name in &["evil.txt.", "evil.txt "] {
        let td = tempdir().unwrap();
        let zip = td.path().join("trailing.zip");
        write_zip_with(name, b"x", &zip);
        let err = extract_with_defaults(&zip, &td.path().join("out")).unwrap_err();
        assert!(
            matches!(err, OtterzipError::PathTraversalBlocked(_)),
            "trailing dot/space {name:?} must be blocked, got {err:?}"
        );
    }
}

#[test]
fn rejects_control_characters_in_path() {
    let td = tempdir().unwrap();
    let zip = td.path().join("ctrl.zip");
    write_zip_with("ev\x07il.txt", b"x", &zip);

    let err = extract_with_defaults(&zip, &td.path().join("out")).unwrap_err();
    assert!(
        matches!(err, OtterzipError::PathTraversalBlocked(_)),
        "control chars must be blocked, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Cumulative bomb gates
// ---------------------------------------------------------------------------

fn build_many_small_bomb(zip: &std::path::Path) {
    let file = fs::File::create(zip).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    let zeros = vec![0u8; 256 * 1024]; // 256 KiB of zeros — DEFLATE shrinks to ~1 KiB.
    for i in 0..32u32 {
        writer.start_file(format!("e/{i:03}.bin"), opts).unwrap();
        writer.write_all(&zeros).unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn cumulative_ratio_blocks_many_small_bomb() {
    let td = tempdir().unwrap();
    let zip = td.path().join("many.zip");
    build_many_small_bomb(&zip);

    // Per-entry ratio is fine (256 KiB / ~1 KiB = ~256, but each entry's
    // raw bytes are below the per-entry default of 1000). The aggregate
    // (32×256 KiB ≈ 8 MiB uncompressed / ~32 KiB compressed ≈ 256:1)
    // blows past this explicit 100:1 cumulative gate.
    //
    // Pinned explicitly rather than via Default: the shipped default was
    // raised to 1000 (it was tripping on legitimately-compressible archives
    // like logs), so ~256:1 no longer trips the default. This test exercises
    // the gate *mechanism* at a fixed threshold, independent of the default.
    let opts = ExtractOptions {
        destination: td.path().join("out"),
        overwrite: OverwritePolicy::Always,
        // Per-entry off, cumulative on at an explicit 100:1.
        max_compression_ratio: 0,
        max_total_compression_ratio: 100,
        ..Default::default()
    };
    let archive = Archive::open(&zip, OpenMode::Read).unwrap();
    let err = archive
        .extract_all::<fn(&Progress) -> bool>(&opts, None)
        .unwrap_err();
    assert!(
        matches!(err, OtterzipError::ZipBombSuspected { .. }),
        "cumulative gate must trip, got {err:?}"
    );
}

#[test]
fn absolute_byte_cap_blocks_oversized_payload() {
    let td = tempdir().unwrap();
    let zip = td.path().join("big.zip");
    build_many_small_bomb(&zip);

    let opts = ExtractOptions {
        destination: td.path().join("out"),
        overwrite: OverwritePolicy::Always,
        max_compression_ratio: 0,
        max_total_compression_ratio: 0,
        // Cap at 1 MiB — fixture writes ~8 MiB → must trip.
        max_total_output_bytes: 1 * 1024 * 1024,
        ..Default::default()
    };
    let archive = Archive::open(&zip, OpenMode::Read).unwrap();
    let err = archive
        .extract_all::<fn(&Progress) -> bool>(&opts, None)
        .unwrap_err();
    assert!(
        matches!(err, OtterzipError::ZipBombSuspected { .. }),
        "absolute byte cap must trip, got {err:?}"
    );
}

#[test]
fn legitimate_archive_passes_phase7_gates() {
    // Sanity: a normal small archive with safe names + low ratio must
    // sail through every Phase 7 check using the documented defaults.
    let td = tempdir().unwrap();
    let zip = td.path().join("ok.zip");
    {
        let file = fs::File::create(&zip).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("README.md", opts).unwrap();
        writer.write_all(b"# hello\nplain text\n").unwrap();
        writer.start_file("docs/api.txt", opts).unwrap();
        writer.write_all(b"reasonable contents\n").unwrap();
        writer.finish().unwrap();
    }

    let opts = ExtractOptions {
        destination: td.path().join("out"),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let archive = Archive::open(&zip, OpenMode::Read).unwrap();
    let report = archive
        .extract_all::<fn(&Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(report.entries_extracted, 2);
}
