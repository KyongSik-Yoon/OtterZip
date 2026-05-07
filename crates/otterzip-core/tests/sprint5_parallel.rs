//! Sprint 5 integration tests: parallel ZIP extraction, password retry
//! flow, and progress cancellation under rayon.

use std::fs;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

use otterzip_core::{
    Archive, ArchiveFormat, ExtractOptions, OpenMode, OverwritePolicy, Progress, ProgressSink,
};
use tempfile::tempdir;

fn build_parallel_zip(out: &std::path::Path) -> u64 {
    use zip::write::SimpleFileOptions;
    let file = fs::File::create(out).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let mut total: u64 = 0;
    // 32 entries × 256 KiB → ~8 MiB uncompressed (passes both
    // PARALLEL_MIN_BYTES and avg-size thresholds).
    let chunk: Vec<u8> = (0..256 * 1024u32)
        .map(|i| b'A' + ((i % 26) as u8))
        .collect();
    for i in 0..32u32 {
        writer.start_file(format!("p/{i:03}.txt"), opts).unwrap();
        writer.write_all(&chunk).unwrap();
        total += chunk.len() as u64;
    }
    writer.finish().unwrap();
    total
}

#[test]
fn parallel_extract_recovers_full_payload() {
    let td = tempdir().unwrap();
    let zip_path = td.path().join("p.zip");
    let total = build_parallel_zip(&zip_path);
    assert_eq!(total, 32 * 256 * 1024);

    let out = td.path().join("out");
    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        // Sprint 5 fixture is intentionally compressible (a..z repeated)
        // so the aggregate ratio exceeds the Phase 7 default 100. The
        // round-trip / progress / serial-fallback intent of these tests
        // is unrelated to the cumulative bomb gate — disable it for the
        // duration of the test only.
        max_total_compression_ratio: 0,
        max_total_output_bytes: 0,
        ..Default::default()
    };

    let archive = Archive::open(&zip_path, OpenMode::Read).unwrap();
    let report = archive
        .extract_all::<fn(&Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(report.entries_extracted, 32);
    assert_eq!(report.bytes_written, total);

    // Every file present + correct length.
    for i in 0..32u32 {
        let p = out.join("p").join(format!("{i:03}.txt"));
        assert!(p.exists(), "missing {p:?}");
        assert_eq!(fs::metadata(&p).unwrap().len(), 256 * 1024);
    }
}

#[test]
fn parallel_extract_progress_callback_fires() {
    // The parallel path updates the sink only every 8 entries (per the
    // comment in zip.rs::extract_all_parallel). With 32 entries we
    // expect at least one tick.
    let td = tempdir().unwrap();
    let zip_path = td.path().join("p.zip");
    build_parallel_zip(&zip_path);

    let counter = AtomicU32::new(0);
    let mut sink = TickCounter(&counter);

    let out = td.path().join("out");
    let opts = ExtractOptions {
        destination: out,
        overwrite: OverwritePolicy::Always,
        max_total_compression_ratio: 0,
        max_total_output_bytes: 0,
        ..Default::default()
    };
    let archive = Archive::open(&zip_path, OpenMode::Read).unwrap();
    let _ = archive.extract_all(&opts, Some(&mut sink)).unwrap();
    assert!(counter.load(Ordering::Relaxed) > 0, "expected at least one progress tick");
}

struct TickCounter<'a>(&'a AtomicU32);
impl<'a> ProgressSink for TickCounter<'a> {
    fn update(&mut self, _progress: &Progress) -> bool {
        self.0.fetch_add(1, Ordering::Relaxed);
        true
    }
}

#[test]
fn parallel_extract_path_traversal_still_blocked() {
    // Build a parallel-eligible archive that ALSO has one malicious
    // entry. The traversal guard must fire even on the parallel path.
    use zip::write::SimpleFileOptions;
    let td = tempdir().unwrap();
    let zip_path = td.path().join("evil.zip");
    {
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let chunk: Vec<u8> = (0..256 * 1024u32)
            .map(|i| b'A' + ((i % 26) as u8))
            .collect();
        for i in 0..16u32 {
            writer.start_file(format!("ok/{i:03}.txt"), opts).unwrap();
            writer.write_all(&chunk).unwrap();
        }
        // Attacker entry — must be blocked.
        writer.start_file("../escape.bin", opts).unwrap();
        writer.write_all(&chunk).unwrap();
        writer.finish().unwrap();
    }

    let out = td.path().join("out");
    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        block_path_traversal: true,
        // Disable cumulative bomb gate — this test's intent is the
        // traversal check; the synthetic fixture's compressibility is
        // incidental.
        max_total_compression_ratio: 0,
        max_total_output_bytes: 0,
        ..Default::default()
    };
    let archive = Archive::open(&zip_path, OpenMode::Read).unwrap();
    let err = archive
        .extract_all::<fn(&Progress) -> bool>(&opts, None)
        .unwrap_err();
    assert!(matches!(
        err,
        otterzip_core::OtterzipError::PathTraversalBlocked(_)
    ));
    // Nothing was leaked outside the destination root.
    assert!(!td.path().join("escape.bin").exists());
}

#[test]
fn parallel_threshold_falls_back_to_serial_for_small_archives() {
    // Small archive (1 small entry) should not engage parallel — the
    // serial path runs and still produces correct output. We assert
    // round-trip rather than which path was taken (private detail).
    use zip::write::SimpleFileOptions;
    let td = tempdir().unwrap();
    let zip_path = td.path().join("small.zip");
    {
        let file = fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("hello.txt", opts).unwrap();
        writer.write_all(b"hello\n").unwrap();
        writer.finish().unwrap();
    }
    let out = td.path().join("out");
    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        // Sprint 5 fixture is intentionally compressible (a..z repeated)
        // so the aggregate ratio exceeds the Phase 7 default 100. The
        // round-trip / progress / serial-fallback intent of these tests
        // is unrelated to the cumulative bomb gate — disable it for the
        // duration of the test only.
        max_total_compression_ratio: 0,
        max_total_output_bytes: 0,
        ..Default::default()
    };
    let archive = Archive::open(&zip_path, OpenMode::Read).unwrap();
    assert_eq!(archive.format(), ArchiveFormat::Zip);
    let report = archive
        .extract_all::<fn(&Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(report.entries_extracted, 1);
    assert_eq!(fs::read(out.join("hello.txt")).unwrap(), b"hello\n");
}
