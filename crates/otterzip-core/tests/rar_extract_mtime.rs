//! T1-4 (RAR): RAR extraction must NOT stamp the on-disk file with the current
//! time — it must carry the archived modification time forward.
//!
//! Unlike ZIP/tar/7z (where `preserve_timestamps` was inert and every extracted
//! file got "now" — the data-loss defect T1-4 fixes with `__apply_extract_mtime`),
//! the RAR backend gets this for free: `unrar`'s `extract_to` (RAR_EXTRACT) sets
//! the archived mtime on the slot, and the scratch→`fs::rename` harvest is a
//! metadata move that preserves it. This test guards that property so a future
//! `unrar` upgrade that silently switched to stamping "now" would be caught.
use otterzip_core::{Archive, ExtractOptions, OpenMode, Progress, ProgressSink};
use std::time::{SystemTime, UNIX_EPOCH};

struct NullSink;
impl ProgressSink for NullSink {
    fn update(&mut self, _p: &Progress) -> bool {
        true
    }
}

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rar")
        .join(name)
}

#[test]
fn rar_extract_carries_archived_mtime_not_now() {
    let src = fixture("version.rar");
    if !src.exists() {
        eprintln!("no fixture, skip");
        return;
    }

    let dst = tempfile::tempdir().unwrap();
    let mut opts = ExtractOptions::default();
    opts.preserve_timestamps = true;
    opts.destination = dst.path().to_path_buf();
    Archive::open(&src, OpenMode::Read)
        .expect("open")
        .extract_all::<NullSink>(&opts, None)
        .expect("extract");

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let files = walkdir_files(dst.path());
    assert!(!files.is_empty(), "expected at least one extracted file");
    for f in files {
        let m = std::fs::metadata(&f)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // |now - m|, computed without wrapping (both directions saturate).
        let abs_delta = now.max(m) - now.min(m);
        // The fixture's VERSION entry is from 2015; "now" is years later. Any
        // gap larger than a year proves the archived time survived rather than
        // being replaced by the wall clock at extraction.
        const ONE_YEAR_SECS: u64 = 365 * 24 * 60 * 60;
        assert!(
            abs_delta > ONE_YEAR_SECS,
            "extracted {:?} got mtime {} which is within a year of now ({}); \
             the archived timestamp was lost and replaced with 'now'",
            f.file_name().unwrap(),
            m,
            now
        );
    }
}

fn walkdir_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
    }
    out
}
