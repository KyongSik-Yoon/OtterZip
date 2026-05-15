//! End-to-end smoke compressor — exercises the SAME `Archive::create` /
//! `add_file` / `add_dir_recursive` / `commit` path the WinUI3 GUI calls
//! through FFI, just without the GUI.
//!
//! Usage:
//!     compress_smoke <input-file-or-dir> <output.zip>
//!
//! Respects `OTTERZIP_PIGZ=1` for the large-entry pigz path (since that's
//! what `writer.rs` reads). Reports wall-clock + total compressed bytes
//! so a quick A/B (env on vs off) is visible.

use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

use otterzip_core::progress::{Progress, ProgressSink};
use otterzip_core::{Archive, ArchiveFormat, CreateOptions};

struct NoopSink;
impl ProgressSink for NoopSink {
    fn update(&mut self, _p: &Progress) -> bool {
        true
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: compress_smoke <input> <output.zip>");
        std::process::exit(2);
    }
    let input = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);

    let pigz_on = env::var("OTTERZIP_PIGZ").ok().as_deref() == Some("1");
    println!("=== compress_smoke ===");
    println!("input : {}", input.display());
    println!("output: {}", output.display());
    println!(
        "OTTERZIP_PIGZ = {}",
        if pigz_on { "1 (pigz enabled for ≥256 MiB entries)" } else { "unset (streaming path)" }
    );

    if !input.exists() {
        eprintln!("error: input does not exist");
        std::process::exit(1);
    }
    if output.exists() {
        std::fs::remove_file(&output).expect("clobber existing output");
    }

    // Pick format from output extension.
    let mut opts = CreateOptions::default();
    let ext = output
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    opts.format = match ext.as_str() {
        "7z" => ArchiveFormat::SevenZ,
        "tar" => ArchiveFormat::Tar,
        "gz" | "tgz" => ArchiveFormat::TarGz,
        _ => ArchiveFormat::Zip,
    };
    println!("format: {:?}  (level={})", opts.format, opts.compression_level);
    let mut archive = Archive::create(&output, opts).expect("Archive::create");

    let started = Instant::now();

    if input.is_dir() {
        let mut sink = NoopSink;
        archive
            .add_dir_recursive(&input, "", Some(&mut sink))
            .expect("add_dir_recursive");
    } else {
        let name = input
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| "entry".to_owned());
        archive.add_file(&input, &name).expect("add_file");
    }

    archive.commit().expect("commit");
    let elapsed = started.elapsed();

    let out_size = std::fs::metadata(&output).expect("stat output").len();
    let in_size = if input.is_dir() {
        walk_dir_size(&input)
    } else {
        std::fs::metadata(&input).expect("stat input").len()
    };

    let mb_in = in_size as f64 / (1024.0 * 1024.0);
    let mb_out = out_size as f64 / (1024.0 * 1024.0);
    let throughput = mb_in / elapsed.as_secs_f64();
    println!();
    println!("=== Result ===");
    println!("input size : {in_size} bytes ({mb_in:.2} MiB)");
    println!("output size: {out_size} bytes ({mb_out:.2} MiB)");
    println!("ratio      : {:.3}", out_size as f64 / in_size as f64);
    println!("elapsed    : {:.2} s", elapsed.as_secs_f64());
    println!("throughput : {:.2} MiB/s (input bytes / wall-clock)", throughput);
}

fn walk_dir_size(p: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(rd) = std::fs::read_dir(p) {
        for ent in rd.flatten() {
            let path = ent.path();
            if path.is_dir() {
                total += walk_dir_size(&path);
            } else if let Ok(m) = std::fs::metadata(&path) {
                total += m.len();
            }
        }
    }
    total
}
