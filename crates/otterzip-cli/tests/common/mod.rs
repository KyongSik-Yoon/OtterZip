//! Shared helpers for the otterzip CLI integration + interop test suites.
//! Each test binary (`cli.rs`, `interop_7zip.rs`) uses a subset, so unused
//! items are expected.
#![allow(dead_code)]

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A fresh `otterzip-cli` command bound to the built binary.
pub fn otterzip() -> Command {
    Command::cargo_bin("otterzip-cli").expect("otterzip-cli binary built")
}

/// Deterministic incompressible bytes (xorshift) — so split archives really
/// split (compressed size stays large) and round-trips exercise real data
/// rather than trivially-compressible zeros.
pub fn incompressible(seed: u32, len: usize) -> Vec<u8> {
    let mut s = if seed == 0 { 0x9E37_79B9 } else { seed };
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s & 0xff) as u8
        })
        .collect()
}

/// Build a small multi-file source tree: a text file, a nested incompressible
/// blob (large enough to force a split at small volume sizes), and a
/// Unicode-named file. Returns `(tempdir, source_root, expected_contents)`
/// where `expected_contents` is the sorted file-content multiset for
/// round-trip equality (path-independent).
pub fn make_sources() -> (TempDir, PathBuf, Vec<Vec<u8>>) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("readme.txt"), b"OtterZip round-trip \x00\x01\x02 test").unwrap();
    fs::write(src.join("sub").join("blob.bin"), incompressible(7, 48 * 1024)).unwrap();
    fs::write(src.join("한글.dat"), "유니코드 이름 페이로드".as_bytes()).unwrap();
    let expected = collect_files(&src);
    (dir, src, expected)
}

/// Every regular-file's content under `dir`, sorted — a path-independent
/// multiset used to assert a round-trip preserved all file contents.
pub fn collect_files(dir: &Path) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    collect_into(dir, &mut out);
    out.sort();
    out
}

fn collect_into(dir: &Path, out: &mut Vec<Vec<u8>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_into(&path, out);
        } else if let Ok(bytes) = fs::read(&path) {
            out.push(bytes);
        }
    }
}

/// Create `format` from the standard source tree, extract it again, and
/// assert the extracted contents are byte-identical. `password` exercises the
/// AES path; `volume` exercises raw byte-split (extracts from the `.001`
/// segment and asserts at least two segments were produced).
pub fn assert_roundtrip(format: &str, password: Option<&str>, volume: Option<&str>) {
    let (dir, src, expected) = make_sources();
    let archive = dir.path().join(format!("out.{format}"));

    let mut create = otterzip();
    create.arg("a").arg(&archive).args(["--format", format]);
    if let Some(pw) = password {
        create.args(["-p", pw]);
    }
    if let Some(v) = volume {
        // store level so the (incompressible) payload stays above the volume
        // size and actually splits.
        create.args(["-v", v, "-l", "0"]);
    }
    create.arg(&src).assert().success();

    let extract_input = match volume {
        Some(_) => {
            let first = dir.path().join(format!("out.{format}.001"));
            assert!(first.exists(), "{format}: first split volume (.001) missing");
            assert!(
                dir.path().join(format!("out.{format}.002")).exists(),
                "{format}: expected the payload to split into >= 2 volumes"
            );
            first
        }
        None => {
            assert!(archive.exists(), "{format}: archive was not created");
            archive.clone()
        }
    };

    let out = dir.path().join("extracted");
    let mut extract = otterzip();
    extract.arg("x").arg(&extract_input).arg("-o").arg(&out).arg("--aoa");
    if let Some(pw) = password {
        extract.args(["-p", pw]);
    }
    extract.assert().success();

    assert_eq!(
        collect_files(&out),
        expected,
        "{format}: round-trip content mismatch (password={password:?}, volume={volume:?})"
    );
}

/// Locate a 7-Zip CLI (`7z.exe` / `7za.exe`) for the interop suite, or `None`
/// when 7-Zip isn't installed (those tests then skip).
pub fn find_7zip() -> Option<PathBuf> {
    for candidate in [
        r"C:\Program Files\7-Zip\7z.exe",
        r"C:\Program Files (x86)\7-Zip\7z.exe",
    ] {
        let p = Path::new(candidate);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in ["7z.exe", "7za.exe"] {
                let p = dir.join(name);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}
