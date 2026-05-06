//! Extraction benchmarks. See `performance.md` §6.
//!
//! Track R skeleton (Sprint 2). Synthesizes an in-memory ZIP corpus so the
//! bench is hermetic and CI-friendly. Real Silesia/enwik9 corpora plug in
//! later via the `SPANZIP_BENCH_CORPUS` env var:
//!   - `SPANZIP_BENCH_CORPUS=/path/to/silesia.zip`
//! When unset (CI default) the synthetic 8 MiB / 1024-entry fixture runs.
//!
//! Regression gate: see `performance.md` §6.4 (PR -5%, main -2%).

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tempfile::TempDir;

use spanzip_core::{Archive, ExtractOptions, OpenMode, OverwritePolicy, Progress};

/// Synthesize a ZIP that exercises both small-entry overhead and bulk
/// throughput. 1024 small text entries + 1 large ~8 MiB binary blob.
fn build_synthetic_zip(out: &std::path::Path) -> u64 {
    let file = fs::File::create(out).expect("create bench fixture");
    let mut writer = zip::ZipWriter::new(file);
    let stored = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut total: u64 = 0;

    // 1024 small deflated text entries — exercises iteration/dispatch.
    let mut buf = String::with_capacity(256);
    for i in 0..1024u32 {
        buf.clear();
        for _ in 0..16 {
            buf.push_str("the quick brown fox jumps over the lazy dog ");
        }
        let name = format!("small/{i:04}.txt");
        writer.start_file(&name, deflated).unwrap();
        writer.write_all(buf.as_bytes()).unwrap();
        total += buf.len() as u64;
    }

    // ~8 MiB stored binary blob — exercises bulk I/O / write path.
    let blob: Vec<u8> = (0..8 * 1024 * 1024u32).map(|i| (i & 0xff) as u8).collect();
    writer.start_file("large/blob.bin", stored).unwrap();
    writer.write_all(&blob).unwrap();
    total += blob.len() as u64;

    writer.finish().unwrap();
    total
}

/// Build a parallel-friendly ZIP: many medium entries (each large enough to
/// give DEFLATE a real workout, small enough that dispatch matters). Sized
/// to clear `PARALLEL_MIN_BYTES` (4 MiB compressed) so the parallel path
/// in `ZipBackend::extract_all_streaming` engages.
fn build_parallel_zip(out: &std::path::Path) -> u64 {
    let file = fs::File::create(out).expect("create parallel fixture");
    let mut writer = zip::ZipWriter::new(file);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut total: u64 = 0;
    // 64 entries × 256 KiB = 16 MiB uncompressed. Pseudo-text content so
    // DEFLATE actually compresses (real-world ratio).
    let chunk: Vec<u8> = (0..256 * 1024u32)
        .map(|i| {
            // Repeating English alphabet — gives ~50% compression.
            b'a' + ((i % 26) as u8)
        })
        .collect();
    for i in 0..64u32 {
        writer.start_file(format!("e/{i:03}.txt"), deflated).unwrap();
        writer.write_all(&chunk).unwrap();
        total += chunk.len() as u64;
    }
    writer.finish().unwrap();
    total
}

fn fixture_path() -> (Option<TempDir>, PathBuf, u64) {
    if let Ok(env_path) = std::env::var("SPANZIP_BENCH_CORPUS") {
        let p = PathBuf::from(env_path);
        let size = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        return (None, p, size);
    }
    let td = tempfile::tempdir().expect("tempdir");
    let zip = td.path().join("synthetic.zip");
    let total = build_synthetic_zip(&zip);
    (Some(td), zip, total)
}

fn bench_extract_all(c: &mut Criterion) {
    let (_holder, zip_path, total_bytes) = fixture_path();

    let mut group = c.benchmark_group("extract_all");
    group.throughput(Throughput::Bytes(total_bytes));
    group.sample_size(10);

    group.bench_with_input(
        BenchmarkId::from_parameter(total_bytes),
        &zip_path,
        |b, path| {
            b.iter_with_setup(
                || tempfile::tempdir().expect("out dir"),
                |out| {
                    let opts = ExtractOptions {
                        destination: out.path().to_path_buf(),
                        overwrite: OverwritePolicy::Always,
                        ..Default::default()
                    };
                    let archive = Archive::open(path, OpenMode::Read).expect("open");
                    let report = archive
                        .extract_all::<fn(&Progress) -> bool>(&opts, None)
                        .expect("extract");
                    std::hint::black_box(report);
                },
            );
        },
    );

    group.finish();
}

fn bench_iterate_entries(c: &mut Criterion) {
    let (_holder, zip_path, _total) = fixture_path();
    let archive = Archive::open(&zip_path, OpenMode::Read).expect("open");

    c.bench_function("iterate_entries", |b| {
        b.iter(|| {
            let mut count: u64 = 0;
            for entry in archive.entries().unwrap() {
                let e = entry.unwrap();
                std::hint::black_box(&e.path);
                count += 1;
            }
            std::hint::black_box(count);
        });
    });
}

/// Engages the rayon parallel path in `ZipBackend::extract_all_streaming`.
/// Use the throughput delta vs `extract_all` (which falls back to serial
/// because the synthetic fixture is below the parallel threshold) as a
/// rough cores-vs-throughput probe.
fn bench_extract_all_parallel(c: &mut Criterion) {
    let td = tempfile::tempdir().expect("tempdir");
    let zip_path = td.path().join("parallel.zip");
    let total_bytes = build_parallel_zip(&zip_path);

    let mut group = c.benchmark_group("extract_all_parallel");
    group.throughput(Throughput::Bytes(total_bytes));
    group.sample_size(10);

    group.bench_with_input(
        BenchmarkId::from_parameter(total_bytes),
        &zip_path,
        |b, path| {
            b.iter_with_setup(
                || tempfile::tempdir().expect("out dir"),
                |out| {
                    let opts = ExtractOptions {
                        destination: out.path().to_path_buf(),
                        overwrite: OverwritePolicy::Always,
                        ..Default::default()
                    };
                    let archive = Archive::open(path, OpenMode::Read).expect("open");
                    let report = archive
                        .extract_all::<fn(&Progress) -> bool>(&opts, None)
                        .expect("extract");
                    std::hint::black_box(report);
                },
            );
        },
    );

    group.finish();
}

criterion_group!(
    benches,
    bench_extract_all,
    bench_iterate_entries,
    bench_extract_all_parallel
);
criterion_main!(benches);
