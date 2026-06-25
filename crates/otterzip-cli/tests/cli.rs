//! Integration tests for the `otterzip` CLI — spawn the built binary and
//! exercise round-trip + error paths end-to-end.
//!
//! The `matrix_*` tests are a black-box format × scenario sweep (plain /
//! AES password / raw byte-split) over the shipped binary — the layer that
//! catches surface-wiring bugs the core's own tests can't (e.g. the CLI not
//! routing a `.001` split volume through the merge path).

mod common;

use common::{assert_roundtrip, make_sources, otterzip};
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

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

// ── L2: black-box format × scenario matrix ──────────────────────────────
// create → extract → byte-identical, through the shipped CLI.

// Plain round-trips for every container create format.
#[test] fn matrix_zip_plain() { assert_roundtrip("zip", None, None); }
#[test] fn matrix_7z_plain() { assert_roundtrip("7z", None, None); }
#[test] fn matrix_tar_plain() { assert_roundtrip("tar", None, None); }
#[test] fn matrix_tar_gz_plain() { assert_roundtrip("tar.gz", None, None); }
#[test] fn matrix_zipx_plain() { assert_roundtrip("zipx", None, None); }

// tar.bz2 / tar.xz *creation* is intentionally deferred (core returns
// FeatureDisabled). Lock that contract so re-enabling it is a deliberate,
// tested change — and so the matrix above doesn't silently rot if support
// lands. (Both formats are still supported for EXTRACTION.)
#[test]
fn matrix_tar_bz2_create_is_disabled() {
    let (dir, src, _) = make_sources();
    let archive = dir.path().join("out.tar.bz2");
    otterzip().arg("a").arg(&archive).args(["--format", "tar.bz2"]).arg(&src).assert().failure();
}

#[test]
fn matrix_tar_xz_create_is_disabled() {
    let (dir, src, _) = make_sources();
    let archive = dir.path().join("out.tar.xz");
    otterzip().arg("a").arg(&archive).args(["--format", "tar.xz"]).arg(&src).assert().failure();
}

// AES-256 password round-trips (encryption-capable formats only).
#[test] fn matrix_zip_password() { assert_roundtrip("zip", Some("p@ss w0rd!"), None); }
#[test] fn matrix_7z_password() { assert_roundtrip("7z", Some("p@ss w0rd!"), None); }

// Raw byte-split (volume) round-trips — create `.001..NNN`, extract from .001.
#[test] fn matrix_zip_split() { assert_roundtrip("zip", None, Some("16k")); }
#[test] fn matrix_7z_split() { assert_roundtrip("7z", None, Some("16k")); }

// AES + split combined (7z) — the hardest path.
#[test] fn matrix_7z_password_split() { assert_roundtrip("7z", Some("combo!"), Some("16k")); }

/// xz is a single-stream codec, not a container — round-trip one file.
#[test]
fn matrix_xz_single_stream() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("data.bin");
    let data = common::incompressible(3, 12 * 1024);
    fs::write(&src, &data).unwrap();
    let archive = dir.path().join("out.xz");
    otterzip().arg("a").arg(&archive).args(["--format", "xz"]).arg(&src).assert().success();

    let out = dir.path().join("extracted");
    otterzip().arg("x").arg(&archive).arg("-o").arg(&out).arg("--aoa").assert().success();
    assert!(
        common::collect_files(&out).contains(&data),
        "xz single-stream round-trip must preserve the payload"
    );
}

/// Wrong password must fail (not silently produce garbage / a 0-byte stub).
#[test]
fn matrix_wrong_password_rejected() {
    let (dir, src, _) = make_sources();
    let archive = dir.path().join("enc.zip");
    otterzip().arg("a").arg(&archive).args(["-p", "correct horse"]).arg(&src).assert().success();

    let out = dir.path().join("extracted");
    otterzip()
        .arg("x")
        .arg(&archive)
        .args(["-p", "WRONG"])
        .arg("-o")
        .arg(&out)
        .arg("--aoa")
        .assert()
        .failure();
}
