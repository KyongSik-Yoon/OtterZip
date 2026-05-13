//! Standalone diagnostic — opens a ZIP archive directly with zip crate
//! and prints timing for each phase. Used to isolate whether a hang
//! is in the upstream `zip` crate vs our wrapper logic.
//!
//! When built with `--features libarchive-fallback` (workspace default
//! release config), also probes the OtterZip `Archive::open` path so
//! the malformed-archive → fallback handoff is observable end-to-end.
//!
//! Usage:  cargo run --example probe_archive -- "C:\path\to\archive.zip"

use std::env;
use std::fs::File;
use std::io::BufReader;
use std::time::Instant;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: probe_archive <path-to-zip> [--raw-zip]");
        std::process::exit(2);
    }
    let path = &args[1];
    let probe_raw = args.iter().any(|a| a == "--raw-zip");
    println!("Probing: {path}");

    let t_open = Instant::now();
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("File::open failed: {e}");
            std::process::exit(1);
        }
    };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    println!("File::open ok in {} us — size: {size} bytes ({:.2} GiB)",
        t_open.elapsed().as_micros(),
        size as f64 / 1024.0 / 1024.0 / 1024.0);

    if !probe_raw {
        println!("(skipping raw zip-crate probe; pass --raw-zip to enable)");
        drop(file);
        let t_otter = Instant::now();
        match otterzip_core::Archive::open(path, otterzip_core::OpenMode::Read) {
            Ok(archive) => {
                println!("Archive::open OK in {} ms — format={:?}",
                    t_otter.elapsed().as_millis(),
                    archive.format());
                // Try listing entries.
                let t_entries = Instant::now();
                match archive.entries() {
                    Ok(iter) => {
                        let count: usize = iter
                            .take_while(|e| e.is_ok())
                            .count();
                        println!("entries() yielded {} entries in {} ms",
                            count, t_entries.elapsed().as_millis());
                    }
                    Err(e) => println!("entries() failed: {e:?}"),
                }
            }
            Err(e) => {
                println!("Archive::open FAILED in {} ms: {:?}",
                    t_otter.elapsed().as_millis(), e);
            }
        }
        return;
    }

    let reader = BufReader::new(file);
    let t_zip = Instant::now();
    println!("Calling zip::ZipArchive::new ...");
    let mut archive = match zip::ZipArchive::new(reader) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("ZipArchive::new failed in {} ms: {e:?}",
                t_zip.elapsed().as_millis());
            std::process::exit(1);
        }
    };
    println!("ZipArchive::new ok in {} ms — entry count: {}",
        t_zip.elapsed().as_millis(),
        archive.len());

    let t_iter = Instant::now();
    let mut printed = 0;
    for i in 0..archive.len() {
        let entry = match archive.by_index_raw(i) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("by_index_raw({i}) failed: {e:?}");
                break;
            }
        };
        if printed < 5 {
            println!("  entry[{i}] name={:?} compressed={} uncompressed={}",
                entry.name(),
                entry.compressed_size(),
                entry.size());
            printed += 1;
        }
    }
    println!("Iterated all entries in {} ms",
        t_iter.elapsed().as_millis());
    println!("Total (raw zip crate): {} ms", t_open.elapsed().as_millis());

    println!();
    println!("=== OtterZip Archive::open path (fallback enabled?) ===");
    let t_otter = Instant::now();
    match otterzip_core::Archive::open(path, otterzip_core::OpenMode::Read) {
        Ok(_archive) => {
            println!("Archive::open OK in {} ms", t_otter.elapsed().as_millis());
        }
        Err(e) => {
            println!("Archive::open FAILED in {} ms: {:?}",
                t_otter.elapsed().as_millis(), e);
        }
    }
}
