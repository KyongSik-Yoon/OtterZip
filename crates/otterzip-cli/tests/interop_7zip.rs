//! L3 cross-tool interop: archives created by OtterZip must open in 7-Zip,
//! and 7-Zip's archives must open in OtterZip — plain and AES-256. This is
//! the only layer that catches "technically-valid but other-tool-incompatible"
//! output. Skipped (passes) when 7-Zip (`7z.exe`) isn't installed, so it's
//! safe in CI without the tool; run locally / on a runner that has 7-Zip for
//! real coverage.

mod common;

use common::{collect_files, find_7zip, make_sources, otterzip};
use std::process::Command;

/// Resolve the 7-Zip binary or bail out of the test (skip) when absent.
macro_rules! sevenzip_or_skip {
    () => {
        match find_7zip() {
            Some(p) => p,
            None => {
                eprintln!("SKIP interop: 7-Zip (7z.exe) not installed");
                return;
            }
        }
    };
}

// ── OtterZip creates → 7-Zip extracts ───────────────────────────────────

#[test]
fn interop_otterzip_zip_opens_in_7zip() {
    let sz = sevenzip_or_skip!();
    let (dir, src, expected) = make_sources();
    let archive = dir.path().join("oz.zip");
    otterzip().arg("a").arg(&archive).arg(&src).assert().success();

    let out = dir.path().join("out");
    let ok = Command::new(&sz)
        .arg("x")
        .arg(&archive)
        .arg(format!("-o{}", out.display()))
        .arg("-y")
        .status()
        .expect("spawn 7-Zip")
        .success();
    assert!(ok, "7-Zip failed to extract OtterZip's zip");
    assert_eq!(collect_files(&out), expected);
}

#[test]
fn interop_otterzip_7z_opens_in_7zip() {
    let sz = sevenzip_or_skip!();
    let (dir, src, expected) = make_sources();
    let archive = dir.path().join("oz.7z");
    otterzip().arg("a").arg(&archive).args(["--format", "7z"]).arg(&src).assert().success();

    let out = dir.path().join("out");
    let ok = Command::new(&sz)
        .arg("x")
        .arg(&archive)
        .arg(format!("-o{}", out.display()))
        .arg("-y")
        .status()
        .expect("spawn 7-Zip")
        .success();
    assert!(ok, "7-Zip failed to extract OtterZip's 7z");
    assert_eq!(collect_files(&out), expected);
}

#[test]
fn interop_otterzip_aes_zip_opens_in_7zip() {
    let sz = sevenzip_or_skip!();
    let (dir, src, expected) = make_sources();
    let archive = dir.path().join("oz_enc.zip");
    otterzip().arg("a").arg(&archive).args(["-p", "interoppw123"]).arg(&src).assert().success();

    let out = dir.path().join("out");
    let ok = Command::new(&sz)
        .arg("x")
        .arg(&archive)
        .arg("-pinteroppw123") // 7-Zip: -p<password>, no space
        .arg(format!("-o{}", out.display()))
        .arg("-y")
        .status()
        .expect("spawn 7-Zip")
        .success();
    assert!(ok, "7-Zip failed to extract OtterZip's AES-256 zip");
    assert_eq!(collect_files(&out), expected);
}

// ── 7-Zip creates → OtterZip extracts ───────────────────────────────────

#[test]
fn interop_7zip_zip_opens_in_otterzip() {
    let sz = sevenzip_or_skip!();
    let (dir, src, expected) = make_sources();
    let archive = dir.path().join("sz.zip");
    let ok = Command::new(&sz)
        .arg("a")
        .arg("-tzip")
        .arg(&archive)
        .arg(&src)
        .status()
        .expect("spawn 7-Zip")
        .success();
    assert!(ok, "7-Zip failed to create a zip");

    let out = dir.path().join("out");
    otterzip().arg("x").arg(&archive).arg("-o").arg(&out).arg("--aoa").assert().success();
    assert_eq!(collect_files(&out), expected);
}

#[test]
fn interop_7zip_7z_opens_in_otterzip() {
    let sz = sevenzip_or_skip!();
    let (dir, src, expected) = make_sources();
    let archive = dir.path().join("sz.7z");
    let ok = Command::new(&sz)
        .arg("a")
        .arg("-t7z")
        .arg(&archive)
        .arg(&src)
        .status()
        .expect("spawn 7-Zip")
        .success();
    assert!(ok, "7-Zip failed to create a 7z");

    let out = dir.path().join("out");
    otterzip().arg("x").arg(&archive).arg("-o").arg(&out).arg("--aoa").assert().success();
    assert_eq!(collect_files(&out), expected);
}

#[test]
fn interop_7zip_aes_zip_opens_in_otterzip() {
    let sz = sevenzip_or_skip!();
    let (dir, src, expected) = make_sources();
    let archive = dir.path().join("sz_enc.zip");
    let ok = Command::new(&sz)
        .arg("a")
        .arg("-tzip")
        .arg("-mem=AES256")
        .arg("-pinteroppw123")
        .arg(&archive)
        .arg(&src)
        .status()
        .expect("spawn 7-Zip")
        .success();
    assert!(ok, "7-Zip failed to create an AES-256 zip");

    let out = dir.path().join("out");
    otterzip()
        .arg("x")
        .arg(&archive)
        .args(["-p", "interoppw123"])
        .arg("-o")
        .arg(&out)
        .arg("--aoa")
        .assert()
        .success();
    assert_eq!(collect_files(&out), expected);
}
