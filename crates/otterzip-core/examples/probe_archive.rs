//! Standalone diagnostic — opens a ZIP archive directly with zip crate
//! and prints timing for each phase. Used to isolate whether a hang
//! is in the upstream `zip` crate vs our wrapper logic.
//!
//! Usage:  cargo run --example probe_archive -- "C:\path\to\archive.zip"

use std::env;
use std::fs::File;
use std::io::BufReader;
use std::time::Instant;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: probe_archive <path-to-zip>");
        std::process::exit(2);
    }
    let path = &args[1];
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
    println!("Total: {} ms", t_open.elapsed().as_millis());
}
