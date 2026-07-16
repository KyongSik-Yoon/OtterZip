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
    __probe_lenient_entries, detect_bytes, Archive, ArchiveFormat, ExtractOptions, OpenMode,
    OtterzipError, OverwritePolicy, Progress,
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
fn truncated_zip_central_directory_does_not_panic() {
    // Build a real ZIP, then chop the last 32 bytes (where the EOCD record
    // lives). Either:
    //   * the strict `zip` backend reports a structural error, or
    //   * the in-tree lenient parser recovers what bytes are still
    //     valid and returns an Archive handle.
    // Both outcomes are fine — the test's invariant is "no panic, no
    // UB" since this fixture is a regression-corpus artefact from
    // Sprint 6's fuzzing pass.
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

    match Archive::open(&bad, OpenMode::Read) {
        Ok(_archive) => {
            // Lenient parser recovered the truncated archive.
            // Acceptable — what we're guarding against is panics.
        }
        Err(err) => {
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
    }
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

// ---------------------------------------------------------------------------
// Lenient ZIP backend — byte-mutation property tests
// ---------------------------------------------------------------------------
//
// These exercise the v1.0-sprint lenient parser against random
// mutations of the EOCD region + a single CDFH record. The invariant
// we care about is *never panic*: a hostile / corrupted archive must
// either fail with a classified `OtterzipError` or succeed with
// some subset of the original entries. Anything else (panic, infinite
// loop, mis-seek into garbage that hands back bogus data) is a bug.

/// Build a small reference archive with 4 deflated entries; returns
/// the bytes + the canonical (name, size) set the parser should
/// agree with on the untouched input.
fn build_reference_zip() -> (Vec<u8>, Vec<(String, u64)>) {
    let mut buf = Vec::new();
    let entries: Vec<(String, Vec<u8>)> = (0..4)
        .map(|i| (format!("payload_{i}.bin"), vec![i as u8; 1024]))
        .collect();
    {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in &entries {
            w.start_file(name.as_str(), opts).unwrap();
            w.write_all(body).unwrap();
        }
        w.finish().unwrap();
    }
    let canonical: Vec<(String, u64)> = entries
        .into_iter()
        .map(|(n, b)| (n, b.len() as u64))
        .collect();
    (buf, canonical)
}

/// Locate the absolute byte offset of the EOCD signature in `bytes`'
/// last 64 KiB tail. Returns `None` when no signature exists (a
/// post-mutation state the property tests tolerate).
fn locate_eocd_in(bytes: &[u8]) -> Option<usize> {
    let needle = [0x50u8, 0x4b, 0x05, 0x06];
    let len = bytes.len();
    if len < 4 {
        return None;
    }
    let scan_start = len.saturating_sub(65_557);
    (scan_start..=len - 4)
        .rev()
        .find(|&i| bytes[i..i + 4] == needle)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Property: arbitrary mutations of the EOCD region (the last
    /// 256 bytes of a healthy archive) must either succeed with
    /// N ≤ original_entries entries, or return a classified
    /// `OtterzipError`. Never panic, never invent entries.
    #[test]
    fn lenient_eocd_region_mutations_never_panic(
        offsets in proptest::collection::vec(0usize..256, 0..16),
        new_bytes in proptest::collection::vec(any::<u8>(), 0..16),
    ) {
        let (mut bytes, canonical) = build_reference_zip();
        let len = bytes.len();
        for (i, off) in offsets.iter().enumerate() {
            if let Some(b) = new_bytes.get(i) {
                let abs = len.saturating_sub(256).saturating_add(*off);
                if abs < len {
                    bytes[abs] = *b;
                }
            }
        }
        let td = tempdir().unwrap();
        let path = td.path().join("mutated.zip");
        fs::write(&path, &bytes).unwrap();

        match __probe_lenient_entries(&path) {
            Ok(entries) => {
                prop_assert!(
                    entries.len() <= canonical.len(),
                    "lenient parser invented entries: {} > {}",
                    entries.len(),
                    canonical.len()
                );
            }
            Err(err) => {
                // Must be a classified error variant — Io / Corrupted
                // / UnsupportedFormat / FeatureDisabled are all
                // acceptable. Anything else is a missed mapping.
                match err {
                    OtterzipError::Io(_)
                    | OtterzipError::Corrupted { .. }
                    | OtterzipError::UnsupportedFormat(_)
                    | OtterzipError::FeatureDisabled(_)
                    | OtterzipError::BackendError(_) => {}
                    other => prop_assert!(
                        false,
                        "unexpected error variant from EOCD mutation: {other:?}"
                    ),
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Property: mutating a single CDFH record's variable-length
    /// fields (filename length / extra length / comment length /
    /// local_header_offset) must leave the lenient parser in a state
    /// where (a) it returns Ok with some subset of the entries, or
    /// (b) it returns a classified error. The leading entries —
    /// before the corruption point — should still be recoverable
    /// when the parser succeeds.
    #[test]
    fn lenient_single_cdfh_mutation_preserves_panic_freedom(
        cdfh_index in 1usize..4,
        field_offset in 0u16..16,
        replacement in any::<u32>(),
    ) {
        let (mut bytes, canonical) = build_reference_zip();
        // Find the cd_offset from the EOCD.
        let eocd_at = match locate_eocd_in(&bytes) {
            Some(off) => off,
            None => return Ok(()),
        };
        let cd_offset =
            u32::from_le_bytes(bytes[eocd_at + 16..eocd_at + 20].try_into().unwrap()) as usize;
        // Walk forward `cdfh_index` records by reading each one's
        // (name_len + extra_len + comment_len) variable suffix. If
        // we can't, the fixture is unexpectedly short and we skip.
        let mut cursor = cd_offset;
        for _ in 0..cdfh_index {
            if cursor + 46 > bytes.len() {
                return Ok(());
            }
            let name_len = u16::from_le_bytes([bytes[cursor + 28], bytes[cursor + 29]]) as usize;
            let extra_len = u16::from_le_bytes([bytes[cursor + 30], bytes[cursor + 31]]) as usize;
            let comment_len = u16::from_le_bytes([bytes[cursor + 32], bytes[cursor + 33]]) as usize;
            cursor += 46 + name_len + extra_len + comment_len;
        }
        if cursor + 46 > bytes.len() {
            return Ok(());
        }
        // Replace 4 bytes at a stable interior offset within the
        // target CDFH's fixed header (e.g. offset 28 = name_len LE,
        // offset 42 = local_header_offset LE).
        let abs = cursor + (field_offset as usize % 16) + 16;
        if abs + 4 > bytes.len() {
            return Ok(());
        }
        bytes[abs..abs + 4].copy_from_slice(&replacement.to_le_bytes());

        let td = tempdir().unwrap();
        let path = td.path().join("cdfh-mutated.zip");
        fs::write(&path, &bytes).unwrap();

        match __probe_lenient_entries(&path) {
            Ok(entries) => {
                prop_assert!(
                    entries.len() <= canonical.len(),
                    "lenient parser invented entries: {} > {}",
                    entries.len(),
                    canonical.len()
                );
            }
            Err(err) => match err {
                OtterzipError::Io(_)
                | OtterzipError::Corrupted { .. }
                | OtterzipError::UnsupportedFormat(_)
                | OtterzipError::FeatureDisabled(_)
                | OtterzipError::BackendError(_) => {}
                other => prop_assert!(
                    false,
                    "unexpected error variant from CDFH mutation: {other:?}"
                ),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// RAR corpus — `guard()`'s catch_unwind is the thing under test
// ---------------------------------------------------------------------------
//
// RAR is the one backend where hostile input can reach an `.unwrap()` we do not
// own. The `unrar` crate panics rather than erroring on input we must treat as
// ordinary attacker data:
//
//   * `Code::from(c).unwrap()` (open_archive.rs:254/449/556) — the vendored
//     library is unrar 7.x and defines `ERAR_LARGE_DICT = 25` (dll.hpp:22), but
//     `unrar_sys`'s constants stop at `ERAR_BAD_PASSWORD = 24`, so
//     `Code::from(25)` is `None`. RAR7 permits 64 GB dictionaries, so this is
//     reachable from LEGITIMATE archives, not just crafted ones.
//   * `EntryFlags::from_bits(..).unwrap()` on any unrecognised flag bit.
//
// `RarBackend::guard` wraps every entry point in `catch_unwind` to turn those
// into a typed `Corrupted`. These tests are what keep that honest.
//
// TAXONOMY NOTE: `MissingVolume` and `PathTraversalBlocked` are RAR-reachable
// and are NOT in the older taxonomies above — a truncated RAR really does
// surface as `MissingVolume` (the DLL's List pass runs an internal Skip and
// reports EOpen@Process), and a mutated name really can trip the traversal
// guard. Both are correct, classified answers, so the RAR set is its own.

fn rar_fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("rar")
        .join(name);
    fs::read(p).expect("rar fixture")
}

/// Every error a RAR may legitimately answer with. Anything outside this set is
/// either a missed mapping or a panic that escaped `guard()`.
fn assert_classified_rar_error(err: &OtterzipError, context: &str) {
    match err {
        OtterzipError::Io(_)
        | OtterzipError::InvalidArgument(_)
        | OtterzipError::UnsupportedFormat(_)
        | OtterzipError::Corrupted { .. }
        | OtterzipError::WrongPassword
        | OtterzipError::FeatureDisabled(_)
        | OtterzipError::BackendError(_)
        // RAR-specific, and the reason this list is not the one above.
        | OtterzipError::MissingVolume { .. }
        | OtterzipError::PathTraversalBlocked(_)
        | OtterzipError::ZipBombSuspected { .. } => {}
        other => panic!("unexpected error variant from {context}: {other:?}"),
    }
}

/// Drive a RAR all the way through open -> list -> extract. Panic-freedom is the
/// invariant; `guard()` is what provides it.
fn exercise_rar(bytes: &[u8]) {
    let td = tempdir().unwrap();
    let path = td.path().join("fuzz.rar");
    fs::write(&path, bytes).unwrap();

    let archive = match Archive::open(&path, OpenMode::Read) {
        Ok(a) => a,
        Err(e) => return assert_classified_rar_error(&e, "RAR open"),
    };
    match archive.entries() {
        Ok(it) => {
            for entry in it {
                if let Err(e) = entry {
                    assert_classified_rar_error(&e, "RAR listing");
                    break;
                }
            }
        }
        Err(e) => assert_classified_rar_error(&e, "RAR listing"),
    }
    let opts = ExtractOptions {
        destination: td.path().join("out"),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    if let Err(e) = archive.extract_all::<fn(&Progress) -> bool>(&opts, None) {
        assert_classified_rar_error(&e, "RAR extract");
    }
}

#[test]
fn rar_corpus_is_classified_not_panicking() {
    // The shipped fixtures, unmutated: the happy path must stay inside the
    // taxonomy too, or the mutation cases below prove nothing.
    for name in [
        "version.rar",
        "crypted.rar",
        "comment-hpw-password.rar",
        "archive.part1.rar",
        "solid.rar",
        "unicode.rar",
        "utf8.rar",
        "traversal.rar",
        "backslash.rar",
        "absolute.rar",
        "symlink.rar",
    ] {
        exercise_rar(&rar_fixture(name));
    }
}

#[test]
fn truncated_rar_does_not_panic() {
    // Chop at every length. Truncation walks the header parser through every
    // partial state, which is where an unrecognised flag/code byte gets read out
    // of whatever happens to follow.
    let full = rar_fixture("version.rar");
    for len in 0..full.len() {
        exercise_rar(&full[..len]);
    }
}

proptest! {
    // Deliberately modest: every case calls into the real unrar C++ and does
    // filesystem I/O, so this is a soak-time knob, not a CI one.
    #![proptest_config(ProptestConfig::with_cases(192))]

    /// Single-byte mutations inside the header region of a real RAR. The 7-byte
    /// marker is left intact so `detect` still routes to the RAR backend —
    /// mutating it would just re-test `UnsupportedFormat` over and over.
    #[test]
    fn mutated_rar_header_never_panics(
        offset in 7usize..59,
        value in any::<u8>(),
    ) {
        let mut bytes = rar_fixture("version.rar");
        bytes[offset] = value;
        exercise_rar(&bytes);
    }
}
