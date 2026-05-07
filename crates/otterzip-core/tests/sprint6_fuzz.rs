//! Sprint 6 fuzz-style invariant tests. Uses proptest (already a dev-dep
//! per the workspace Cargo.toml) so we don't need a nightly toolchain or
//! libFuzzer wiring. Coverage is bounded — these are *panic-freedom* and
//! *error-mapping* checks, not full corpus fuzzing. Real fuzz time
//! (cargo-fuzz, 24h soak) lands during the QA cycle described in
//! `docs/01-plan/performance.md` §6.4.

use std::fs;
use std::io::Write;

use proptest::prelude::*;
use otterzip_core::{
    detect_bytes, Archive, ArchiveFormat, ExtractOptions, OpenMode, OverwritePolicy, Progress,
    OtterzipError,
};
use tempfile::tempdir;

/// Property: `detect_bytes` is total — never panics on arbitrary input.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn detect_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..1024)) {
        // The function returns Option<ArchiveFormat>; both arms are valid.
        // We just assert "did not panic". `unwrap_or` collapses both cases.
        let _ = detect_bytes(&bytes).unwrap_or(ArchiveFormat::Unknown);
    }
}

/// Property: opening arbitrary garbage as an archive returns an `Err`,
/// never panics. The shape of the error must be one of the expected
/// taxonomy variants — anything else (e.g. an Io error chain we forgot
/// to map) is a bug we want surfaced.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn open_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..16 * 1024)) {
        let td = tempdir().unwrap();
        let path = td.path().join("fuzz.bin");
        fs::write(&path, &bytes).unwrap();

        match Archive::open(&path, OpenMode::Read) {
            Ok(_) => {
                // Some random byte sequences (e.g. starting with PK\x05\x06)
                // legitimately parse as empty archives. That's fine — the
                // important thing is no panic.
            }
            Err(err) => {
                // Spot-check the error is one we recognise (not a generic
                // Io::Other we forgot to classify).
                match err {
                    OtterzipError::Io(_)
                    | OtterzipError::InvalidArgument(_)
                    | OtterzipError::UnsupportedFormat(_)
                    | OtterzipError::Corrupted { .. }
                    | OtterzipError::WrongPassword
                    | OtterzipError::FeatureDisabled(_)
                    | OtterzipError::BackendError(_) => {}
                    other => prop_assert!(
                        false,
                        "unexpected error variant from random input: {other:?}"
                    ),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Hand-crafted bad samples (regression corpus)
// ---------------------------------------------------------------------------

#[test]
fn truncated_zip_central_directory_returns_error_not_panic() {
    // Build a real ZIP, then chop the last 32 bytes (where the EOCD record
    // lives). The `zip` crate must report a structural error.
    let td = tempdir().unwrap();
    let good = td.path().join("ok.zip");
    {
        let file = fs::File::create(&good).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("a.txt", opts).unwrap();
        writer.write_all(b"hello").unwrap();
        writer.finish().unwrap();
    }
    let bytes = fs::read(&good).unwrap();
    let truncated = &bytes[..bytes.len().saturating_sub(32)];
    let bad = td.path().join("truncated.zip");
    fs::write(&bad, truncated).unwrap();

    let err = Archive::open(&bad, OpenMode::Read).unwrap_err();
    assert!(
        matches!(
            err,
            OtterzipError::Corrupted { .. }
                | OtterzipError::UnsupportedFormat(_)
                | OtterzipError::Io(_)
                | OtterzipError::BackendError(_)
        ),
        "got {err:?}"
    );
}

#[test]
fn zip_with_only_eocd_signature_does_not_panic() {
    // Plain "PK\x05\x06" + zeros — the magic looks like an empty archive
    // but the trailing bytes are absent.
    let td = tempdir().unwrap();
    let path = td.path().join("empty.zip");
    fs::write(&path, [0x50, 0x4B, 0x05, 0x06]).unwrap();
    // Either it succeeds with 0 entries, or it errors — both are acceptable.
    match Archive::open(&path, OpenMode::Read) {
        Ok(a) => {
            // If it opened, iteration must also be safe.
            let count: usize = a.entries().unwrap().count();
            assert_eq!(count, 0);
        }
        Err(_) => {}
    }
}

#[test]
fn extract_into_existing_filtered_destination_safe() {
    // Sprint 6 sanity: the extract API tolerates a destination that was
    // pre-populated with files matching some entries when overwrite=Always.
    let td = tempdir().unwrap();
    let archive_path = td.path().join("seed.zip");
    {
        let file = fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("dup.txt", opts).unwrap();
        writer.write_all(b"new contents\n").unwrap();
        writer.finish().unwrap();
    }
    let out = td.path().join("out");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("dup.txt"), b"old contents\n").unwrap();

    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let archive = Archive::open(&archive_path, OpenMode::Read).unwrap();
    let report = archive
        .extract_all::<fn(&Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(report.entries_extracted, 1);
    assert_eq!(fs::read(out.join("dup.txt")).unwrap(), b"new contents\n");
}
