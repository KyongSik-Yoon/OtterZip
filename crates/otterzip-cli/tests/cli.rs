//! Integration tests for the `otterzip` CLI — spawn the built binary
//! and exercise round-trip + error paths end-to-end.
//!
//! Password/AES-256 ZIP creation is intentionally NOT tested here: the
//! ZIP write-side encryption is a v1.1 item (see settings-catalog), so
//! a wrong-password test would assert against an unimplemented path.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

fn otterzip() -> Command {
    Command::cargo_bin("otterzip").expect("otterzip binary built")
}

#[test]
fn roundtrip_zip_with_korean_name() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("테스트.txt");
    fs::write(&src, b"hello otter").unwrap();
    let archive = dir.path().join("out.zip");

    otterzip().arg("a").arg(&archive).arg(&src).assert().success();
    assert!(archive.exists(), "archive should be created");

    // List shows the (UTF-8) entry name without mojibake.
    otterzip()
        .arg("l")
        .arg(&archive)
        .assert()
        .success()
        .stdout(predicate::str::contains("테스트.txt"));

    // Extract and verify byte-identical content.
    let outdir = dir.path().join("out");
    otterzip()
        .arg("x")
        .arg(&archive)
        .arg("-o")
        .arg(&outdir)
        .assert()
        .success();
    assert_eq!(fs::read(outdir.join("테스트.txt")).unwrap(), b"hello otter");

    // Integrity test passes.
    otterzip().arg("t").arg(&archive).assert().success();
}

#[test]
fn list_missing_archive_is_io_error() {
    let dir = tempdir().unwrap();
    otterzip()
        .arg("l")
        .arg(dir.path().join("nope.zip"))
        .assert()
        .failure()
        .code(3); // Io
}

#[test]
fn create_only_guards_existing_target() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("a.txt");
    fs::write(&src, b"x").unwrap();
    let archive = dir.path().join("out.zip");

    otterzip().arg("a").arg(&archive).arg(&src).assert().success();
    // Second create against the same path must refuse (exit 2).
    otterzip()
        .arg("a")
        .arg(&archive)
        .arg(&src)
        .assert()
        .failure()
        .code(2);
}

#[test]
fn unknown_format_rejected() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("a.txt");
    fs::write(&src, b"x").unwrap();
    otterzip()
        .arg("a")
        .arg(dir.path().join("out.bin"))
        .arg(&src)
        .arg("--format")
        .arg("nope")
        .assert()
        .failure()
        .code(2);
}

#[test]
fn extract_flat_strips_directories() {
    let dir = tempdir().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("inner.txt"), b"deep").unwrap();
    let archive = dir.path().join("nested.zip");

    // storeroot nests under "sub/" inside the archive.
    otterzip()
        .arg("a")
        .arg(&archive)
        .arg(&sub)
        .arg("--storeroot")
        .assert()
        .success();

    // `e` flattens — inner.txt lands directly in the output dir.
    let outdir = dir.path().join("flat");
    otterzip()
        .arg("e")
        .arg(&archive)
        .arg("-o")
        .arg(&outdir)
        .assert()
        .success();
    assert!(outdir.join("inner.txt").exists(), "flatten should drop the sub/ prefix");
}
