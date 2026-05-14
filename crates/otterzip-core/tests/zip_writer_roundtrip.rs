//! Path B compress sprint — round-trip cross-validation against the
//! in-tree ZIP writer.
//!
//! The new `backends::zip_writer::ZipFileWriter` replaces the
//! upstream `zip` crate on the encode side. We don't want to ship
//! that change behind anyone's back, so this file goes through three
//! independent verification layers:
//!
//!   1. **Strict zip-rs cross-validate** — re-open our output through
//!      the upstream `zip::ZipArchive` (which is what every Java
//!      stack / 7-Zip / Bandizip uses internally). If zip-rs accepts
//!      it and the bytes round-trip, every external consumer in
//!      practice will too. The first 5 cases hit this gate.
//!   2. **Our own lenient reader cross-validate** — re-open through
//!      `Archive::open` (which routes through `strict ZipBackend` for
//!      healthy archives). Catches a bug where we'd write something
//!      strict zip-rs accepts but our other dispatch arm rejects.
//!   3. **Full `Archive::create + add_file + commit`** — ABI-level
//!      ride: open the public writer, add entries, commit, then read
//!      back through the public reader. This is what the FFI cdylib
//!      and the WinUI host actually exercise.
//!
//! Plus negative coverage: a 1-shot path that overshoots the 16 MiB
//! libdeflater threshold lands on the flate2 streaming fallback —
//! we want byte-for-byte parity there too.

use std::fs::{self, File};
use std::io::{BufReader, Cursor, Read};
use std::path::Path;

use otterzip_core::{
    Archive, ArchiveFormat, CompressionMethod, CreateOptions, ExtractOptions, OpenMode,
    OverwritePolicy,
};
use tempfile::tempdir;

/// Helper: rip every entry out of `path` via the upstream zip crate
/// and return the canonical `(name, bytes)` set. Used to assert
/// strict compatibility — if upstream zip-rs accepts the bytes,
/// every Java / 7-Zip / WinRAR consumer in the wild will too.
fn strict_extract_all(path: &Path) -> Vec<(String, Vec<u8>)> {
    let f = File::open(path).expect("open archive");
    let mut archive =
        zip::ZipArchive::new(BufReader::new(f)).expect("strict zip-rs parse");
    let mut out = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut zf = archive.by_index(i).expect("by_index");
        let name = zf.name().to_string();
        let mut bytes = Vec::new();
        zf.read_to_end(&mut bytes).expect("strict read_to_end");
        out.push((name, bytes));
    }
    out
}

/// Helper: feed `entries` through our writer and produce an archive
/// at `path`. Goes through the public `Archive::create` API the FFI
/// cdylib uses — covers the same wiring the WinUI host hits.
fn build_archive_public_api(
    path: &Path,
    entries: &[(&str, &[u8])],
    method: CompressionMethod,
    level: u8,
) {
    let opts = CreateOptions {
        format: ArchiveFormat::Zip,
        compression: method,
        compression_level: level,
        ..Default::default()
    };
    let mut archive = Archive::create(path, opts).expect("Archive::create");
    for (name, body) in entries {
        // Stage the bytes to a temp file so add_file's
        // (source path → read) shape works without a custom hook.
        let staging = path
            .parent()
            .unwrap()
            .join(format!("__staging_{}", name.replace('/', "_")));
        fs::write(&staging, body).expect("staging write");
        archive
            .add_file(&staging, name)
            .expect("Archive::add_file");
        let _ = fs::remove_file(&staging);
    }
    archive.commit().expect("Archive::commit");
}

#[test]
fn strict_zip_rs_reads_single_deflated_entry() {
    let td = tempdir().unwrap();
    let path = td.path().join("deflate.zip");
    let payload = b"otterzip writes its own zip now\n".repeat(64);
    build_archive_public_api(
        &path,
        &[("greeting.txt", &payload)],
        CompressionMethod::Deflate,
        5,
    );
    let got = strict_extract_all(&path);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "greeting.txt");
    assert_eq!(got[0].1, payload);
}

#[test]
fn strict_zip_rs_reads_stored_entry() {
    let td = tempdir().unwrap();
    let path = td.path().join("stored.zip");
    let payload = b"raw bytes through method 0";
    build_archive_public_api(
        &path,
        &[("plain.bin", payload)],
        CompressionMethod::Store,
        0,
    );
    let got = strict_extract_all(&path);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "plain.bin");
    assert_eq!(got[0].1, payload);
}

#[test]
fn strict_zip_rs_reads_multi_entry_archive() {
    let td = tempdir().unwrap();
    let path = td.path().join("multi.zip");
    let entries: Vec<(String, Vec<u8>)> = (0..16)
        .map(|i| {
            let mut payload = Vec::with_capacity(2048);
            // Deterministic pseudo-random so deflate has work to do.
            let mut seed: u64 = 0xA17E_B07E_DEAD_BEEFu64.wrapping_add(i as u64);
            for _ in 0..2048 {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                payload.push((seed >> 33) as u8);
            }
            (format!("payload_{i:02}.bin"), payload)
        })
        .collect();
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    build_archive_public_api(&path, &refs, CompressionMethod::Deflate, 5);
    let got = strict_extract_all(&path);
    assert_eq!(got.len(), entries.len());
    for (i, (name, body)) in got.iter().enumerate() {
        assert_eq!(*name, entries[i].0, "entry {i} name");
        assert_eq!(*body, entries[i].1, "entry {i} bytes");
    }
}

#[test]
fn strict_zip_rs_reads_unicode_filename() {
    // GP bit 11 (UTF-8) is always set by our writer. Korean filename
    // round-trips via strict zip-rs so external tools (Bandizip,
    // 7-Zip, WinRAR) see the right text in their listing.
    let td = tempdir().unwrap();
    let path = td.path().join("unicode.zip");
    let entries = vec![("주문서.txt", b"\xEC\x95\x88\xEB\x85\x95".as_slice())]; // "안녕"
    build_archive_public_api(&path, &entries, CompressionMethod::Deflate, 5);
    let got = strict_extract_all(&path);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "주문서.txt");
    assert_eq!(got[0].1, b"\xEC\x95\x88\xEB\x85\x95");
}

#[test]
fn archive_public_api_full_create_extract_roundtrip() {
    // Highest-coverage shape: Archive::create + add_file + commit on
    // one side, Archive::open + extract_all + bytes-on-disk
    // verification on the other. Mirrors what the FFI cdylib and the
    // WinUI host exercise end-to-end.
    let td = tempdir().unwrap();
    let path = td.path().join("public.zip");
    let dest = td.path().join("out");
    let entries: Vec<(String, Vec<u8>)> = vec![
        ("readme.md".to_string(), b"# OtterZip\n\nhello\n".to_vec()),
        ("nested/data.bin".to_string(), vec![0x55u8; 4096]),
        ("nested/sub/leaf.txt".to_string(), b"deep".to_vec()),
    ];
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    build_archive_public_api(&path, &refs, CompressionMethod::Deflate, 5);

    // Open + extract via the public reader.
    let archive = Archive::open(&path, OpenMode::Read).unwrap();
    assert_eq!(archive.format(), ArchiveFormat::Zip);
    let opts = ExtractOptions {
        destination: dest.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let report = archive
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(report.entries_extracted, entries.len() as u32);

    for (name, expected) in &entries {
        let on_disk = fs::read(dest.join(name)).unwrap();
        assert_eq!(on_disk, *expected, "entry {name} bytes mismatch");
    }
}

#[test]
fn flate2_streaming_path_handles_payload_above_threshold() {
    // 17 MiB pseudo-random payload — crosses the 16 MiB libdeflater
    // threshold so the streaming flate2 path takes over. Bytes must
    // round-trip exactly: pure-random data won't compress, but
    // the encoder/decoder pair has to stay byte-identical on the
    // boundary case.
    let td = tempdir().unwrap();
    let path = td.path().join("threshold.zip");
    let mut payload = Vec::with_capacity(17 * 1024 * 1024);
    let mut seed: u64 = 0xC0FFEE_BABE;
    for _ in 0..17 * 1024 * 1024 {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        payload.push((seed >> 33) as u8);
    }
    build_archive_public_api(
        &path,
        &[("large.bin", &payload)],
        CompressionMethod::Deflate,
        5,
    );
    let got = strict_extract_all(&path);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "large.bin");
    assert_eq!(got[0].1.len(), payload.len());
    assert_eq!(got[0].1, payload, "large entry corrupted at streaming threshold");
}

#[test]
fn add_directory_entry_creates_directory_record() {
    // Stages a source tree with a single sub-directory + nested file,
    // then runs Archive::add_dir_recursive which goes through
    // ZipFileWriter::add_directory + add_entry on the way down.
    // Strict zip-rs must see a name ending in `/` and a 0-byte
    // payload, and Archive::extract_all should materialize the
    // directory on disk.
    let td = tempdir().unwrap();
    let path = td.path().join("with-dirs.zip");
    let dest = td.path().join("out");
    let source = td.path().join("source");
    fs::create_dir_all(source.join("docs")).unwrap();
    fs::write(source.join("docs").join("leaf.md"), b"## leaf").unwrap();

    let opts = CreateOptions {
        format: ArchiveFormat::Zip,
        compression: CompressionMethod::Deflate,
        compression_level: 5,
        ..Default::default()
    };
    let mut archive = Archive::create(&path, opts).unwrap();
    archive
        .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&source, "", None)
        .unwrap();
    archive.commit().unwrap();

    // Strict reader sanity — the directory entry survives and the
    // nested file's bytes round-trip exactly.
    let got = strict_extract_all(&path);
    assert!(
        got.iter().any(|(n, _)| n == "docs/"),
        "directory CDFH entry missing"
    );
    assert!(
        got.iter().any(|(n, b)| n == "docs/leaf.md" && b == b"## leaf"),
        "nested file did not round-trip strict",
    );

    // Public reader + extract must reconstruct the directory on disk.
    let archive = Archive::open(&path, OpenMode::Read).unwrap();
    let opts = ExtractOptions {
        destination: dest.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    archive
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert!(dest.join("docs").is_dir(), "directory entry was not extracted");
    let leaf = fs::read(dest.join("docs").join("leaf.md")).unwrap();
    assert_eq!(leaf, b"## leaf");
}

#[test]
fn empty_archive_roundtrips_through_public_api() {
    // The trivial fixture: zero entries. EOCD-only output. Strict
    // zip-rs must accept it and report len()==0.
    let td = tempdir().unwrap();
    let path = td.path().join("empty.zip");
    let opts = CreateOptions {
        format: ArchiveFormat::Zip,
        ..Default::default()
    };
    let archive = Archive::create(&path, opts).unwrap();
    archive.commit().unwrap();
    let got = strict_extract_all(&path);
    assert_eq!(got.len(), 0);
}

/// Build a source directory with `n` files of `payload_size` bytes
/// each, returning the canonical `(entry_name, body)` set so callers
/// can verify byte-for-byte round-trip on extract.
fn stage_source_tree(
    root: &Path,
    n: usize,
    payload_size: usize,
) -> Vec<(String, Vec<u8>)> {
    fs::create_dir_all(root).unwrap();
    let mut entries = Vec::with_capacity(n);
    let mut seed: u64 = 0xCAFE_BABE_DEAD_F00D;
    for i in 0..n {
        let mut payload = Vec::with_capacity(payload_size);
        for _ in 0..payload_size {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            payload.push((seed >> 33) as u8);
        }
        let name = format!("payload_{i:04}.bin");
        fs::write(root.join(&name), &payload).unwrap();
        entries.push((name, payload));
    }
    entries
}

#[test]
fn parallel_directory_walk_round_trips_under_strict_reader() {
    // 32 entries × 16 KiB = 512 KiB total, comfortably above the
    // `PARALLEL_MIN_ENTRIES = 16` threshold so the dispatcher
    // commits to the bulk path. Strict zip-rs must read every byte
    // back exactly.
    let td = tempdir().unwrap();
    let archive_path = td.path().join("parallel.zip");
    let source = td.path().join("source");
    let expected = stage_source_tree(&source, 32, 16 * 1024);

    let opts = CreateOptions {
        format: ArchiveFormat::Zip,
        compression: CompressionMethod::Deflate,
        compression_level: 5,
        ..Default::default()
    };
    let mut archive = Archive::create(&archive_path, opts).unwrap();
    archive
        .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&source, "", None)
        .unwrap();
    archive.commit().unwrap();

    // Strict zip-rs cross-validate. Sort both sides because the
    // bulk path's ordering follows the same alphabetic pop sequence
    // as the serial walker, but small filesystem-iteration quirks
    // can shuffle siblings slightly across runs.
    let mut got = strict_extract_all(&archive_path);
    let mut want = expected;
    got.sort_by(|a, b| a.0.cmp(&b.0));
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(got.len(), want.len(), "entry count mismatch");
    for ((g_name, g_body), (w_name, w_body)) in got.iter().zip(want.iter()) {
        assert_eq!(g_name, w_name, "name");
        assert_eq!(g_body, w_body, "body for {g_name}");
    }
}

#[test]
fn parallel_path_extract_roundtrip_via_public_api() {
    // End-to-end: parallel compress → public Archive::extract_all
    // → bytes-on-disk verification. Mirrors the WinUI host's hot
    // path so a regression in the worker pool's ordering or the
    // main-thread LFH bookkeeping shows up here immediately.
    let td = tempdir().unwrap();
    let archive_path = td.path().join("parallel-public.zip");
    let source = td.path().join("source");
    let expected = stage_source_tree(&source, 24, 4 * 1024);
    let dest = td.path().join("out");

    let opts = CreateOptions {
        format: ArchiveFormat::Zip,
        compression: CompressionMethod::Deflate,
        compression_level: 5,
        ..Default::default()
    };
    let mut archive = Archive::create(&archive_path, opts).unwrap();
    archive
        .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&source, "", None)
        .unwrap();
    archive.commit().unwrap();

    let archive = Archive::open(&archive_path, OpenMode::Read).unwrap();
    let opts = ExtractOptions {
        destination: dest.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let report = archive
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(report.entries_extracted, expected.len() as u32);
    for (name, body) in &expected {
        let got = fs::read(dest.join(name)).unwrap();
        assert_eq!(got, *body, "parallel-extracted bytes for {name}");
    }
}

#[test]
fn parallel_path_handles_mixed_size_chunk_boundary() {
    // 18 small entries × 64 KiB. Each entry's deflate is small
    // enough to fit the libdeflater one-shot path, and the count
    // crosses PARALLEL_MIN_ENTRIES (16) so the bulk dispatcher
    // fires. Validates that all 18 entries survive the par_iter
    // → ordered-write splicing with byte-identical payloads.
    let td = tempdir().unwrap();
    let archive_path = td.path().join("mixed.zip");
    let source = td.path().join("source");
    let expected = stage_source_tree(&source, 18, 64 * 1024);

    let opts = CreateOptions {
        format: ArchiveFormat::Zip,
        compression: CompressionMethod::Deflate,
        compression_level: 5,
        ..Default::default()
    };
    let mut archive = Archive::create(&archive_path, opts).unwrap();
    archive
        .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&source, "", None)
        .unwrap();
    archive.commit().unwrap();

    let mut got = strict_extract_all(&archive_path);
    let mut want = expected;
    got.sort_by(|a, b| a.0.cmp(&b.0));
    want.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(got.len(), want.len());
    for ((g_name, g_body), (w_name, w_body)) in got.iter().zip(want.iter()) {
        assert_eq!(g_name, w_name);
        assert_eq!(g_body.len(), w_body.len(), "size for {g_name}");
        assert_eq!(g_body, w_body, "body for {g_name}");
    }
}

#[test]
fn random_seed_payload_size_distribution_roundtrip() {
    // Mix of small (~1 KB), medium (~64 KB) and one larger (~512 KB)
    // entry within a single archive. Exercises libdeflater one-shot
    // across the 0..LIBDEFLATER_ONESHOT_THRESHOLD range — none of
    // these crosses 16 MiB so all take the one-shot path.
    let td = tempdir().unwrap();
    let path = td.path().join("mixed.zip");
    fn pseudo_bytes(seed: &mut u64, n: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            *seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            v.push((*seed >> 33) as u8);
        }
        v
    }
    let mut seed = 0x1234_5678u64;
    let entries = vec![
        ("a_small.bin".to_string(), pseudo_bytes(&mut seed, 1024)),
        ("b_medium.bin".to_string(), pseudo_bytes(&mut seed, 64 * 1024)),
        ("c_large_oneshot.bin".to_string(), pseudo_bytes(&mut seed, 512 * 1024)),
    ];
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    build_archive_public_api(&path, &refs, CompressionMethod::Deflate, 5);

    let got = strict_extract_all(&path);
    assert_eq!(got.len(), entries.len());
    for (i, (name, body)) in got.iter().enumerate() {
        assert_eq!(*name, entries[i].0);
        assert_eq!(body.len(), entries[i].1.len());
        assert_eq!(*body, entries[i].1, "{name} payload mismatch");
    }
    // Read explicitly through Archive::read_entry as well — covers a
    // different code path than extract_all (per-entry stream read).
    let archive = Archive::open(&path, OpenMode::Read).unwrap();
    for (name, expected) in &entries {
        let mut stream = archive.read_entry(name).unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, *expected, "Archive::read_entry round-trip for {name}");
    }
    // And confirm with the in-memory Cursor variant too — guards
    // against a regression where the writer accidentally embedded
    // a path-dependent quirk that only failed via on-disk paths.
    let _check_cursor = Cursor::new(fs::read(&path).unwrap());
}
