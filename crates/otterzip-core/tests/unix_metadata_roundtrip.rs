//! POSIX permission and symlink fidelity across a create → extract cycle.
//!
//! These are the Linux port's load-bearing behaviours. Before the port both
//! halves were broken and the breakage cancelled out invisibly: the writers
//! hardcoded `0o644`/`0o755` and the extractor ignored the mode entirely, so a
//! round trip "worked" while silently stripping every execute bit. A test that
//! only checked file CONTENT — which is what the existing round-trip tests do
//! — could not see it.
//!
//! Everything here is `#[cfg(unix)]`: Windows has no mode to preserve, and
//! `CreateSymbolicLinkW` needs a privilege an ordinary user does not have.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use otterzip_core::{
    Archive, ArchiveFormat, CompressionMethod, CreateOptions, ExtractOptions, OpenMode,
    OverwritePolicy, Progress, ProgressSink,
};
use tempfile::tempdir;

struct NullSink;
impl ProgressSink for NullSink {
    fn update(&mut self, _: &Progress) -> bool {
        true
    }
}

fn create_opts(format: ArchiveFormat) -> CreateOptions {
    CreateOptions {
        format,
        compression: CompressionMethod::Deflate,
        compression_level: 6,
        preserve_permissions: true,
        preserve_timestamps: true,
        ..CreateOptions::default()
    }
}

fn extract_opts(dest: &Path) -> ExtractOptions {
    ExtractOptions {
        destination: dest.to_path_buf(),
        overwrite: OverwritePolicy::Always,
        ..ExtractOptions::default()
    }
}

fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Build a source tree with three deliberately different modes.
fn build_tree(root: &Path) {
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(root.join("bin/run.sh"), b"#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(root.join("bin/run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(root.join("readme.txt"), b"plain\n").unwrap();
    fs::set_permissions(root.join("readme.txt"), fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(root.join("private.key"), b"secret\n").unwrap();
    fs::set_permissions(root.join("private.key"), fs::Permissions::from_mode(0o600)).unwrap();
}

fn roundtrip(format: ArchiveFormat, archive_name: &str) {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    build_tree(&src);

    let archive_path = td.path().join(archive_name);
    {
        let mut archive =
            Archive::create(&archive_path, create_opts(format)).expect("create archive");
        archive
            .add_dir_recursive::<NullSink>(&src, "", None)
            .expect("add tree");
        archive.commit().expect("commit");
    }

    let dest = td.path().join("out");
    let archive = Archive::open(&archive_path, OpenMode::Read).expect("open archive");
    archive
        .extract_all::<NullSink>(&extract_opts(&dest), None)
        .expect("extract");

    assert_eq!(
        mode_of(&dest.join("bin/run.sh")),
        0o755,
        "{archive_name}: the execute bit did not survive the round trip"
    );
    assert_eq!(
        mode_of(&dest.join("readme.txt")),
        0o644,
        "{archive_name}: a plain file's mode changed"
    );
    // 0o600 also proves the umask is not being applied on top of a mode that
    // is already more restrictive than it — `& !umask` must only ever remove
    // bits the archive granted.
    assert_eq!(
        mode_of(&dest.join("private.key")),
        0o600,
        "{archive_name}: a private file came out more permissive than archived"
    );
}

#[test]
fn zip_preserves_unix_modes() {
    roundtrip(ArchiveFormat::Zip, "tree.zip");
}

#[test]
fn tar_gz_preserves_unix_modes() {
    roundtrip(ArchiveFormat::TarGz, "tree.tar.gz");
}

/// setuid, setgid and sticky must never come back out of an archive. An
/// archive is untrusted input, and a `04755` entry extracted faithfully is a
/// local privilege-escalation primitive handed to whoever sent the file.
#[test]
fn setuid_bit_is_not_restored() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let evil = src.join("evil");
    fs::write(&evil, b"payload\n").unwrap();
    fs::set_permissions(&evil, fs::Permissions::from_mode(0o4755)).unwrap();
    // Some filesystems (and any non-root user on a `nosuid` mount) refuse to
    // persist the bit. If it did not stick on the SOURCE, the test would pass
    // vacuously, so skip rather than pretend it proved something.
    if fs::metadata(&evil).unwrap().permissions().mode() & 0o4000 == 0 {
        return;
    }

    let archive_path = td.path().join("evil.tar.gz");
    {
        let mut archive = Archive::create(&archive_path, create_opts(ArchiveFormat::TarGz))
            .expect("create archive");
        archive
            .add_dir_recursive::<NullSink>(&src, "", None)
            .expect("add tree");
        archive.commit().expect("commit");
    }

    let dest = td.path().join("out");
    let archive = Archive::open(&archive_path, OpenMode::Read).expect("open archive");
    archive
        .extract_all::<NullSink>(&extract_opts(&dest), None)
        .expect("extract");

    let out_mode = fs::metadata(dest.join("evil")).unwrap().permissions().mode();
    assert_eq!(
        out_mode & 0o7000,
        0,
        "setuid/setgid/sticky must be stripped on extract, got {:o}",
        out_mode
    );
    assert_eq!(out_mode & 0o777, 0o755, "the permission bits should survive");
}

/// `preserve_permissions: false` must actually turn the feature off, leaving
/// files at whatever the platform default produces.
#[test]
fn preserve_permissions_false_leaves_modes_alone() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    build_tree(&src);

    let archive_path = td.path().join("tree.tar.gz");
    {
        let mut archive = Archive::create(&archive_path, create_opts(ArchiveFormat::TarGz))
            .expect("create archive");
        archive
            .add_dir_recursive::<NullSink>(&src, "", None)
            .expect("add tree");
        archive.commit().expect("commit");
    }

    let dest = td.path().join("out");
    let opts = ExtractOptions {
        preserve_permissions: false,
        ..extract_opts(&dest)
    };
    let archive = Archive::open(&archive_path, OpenMode::Read).expect("open archive");
    archive.extract_all::<NullSink>(&opts, None).expect("extract");

    assert_eq!(
        mode_of(&dest.join("bin/run.sh")) & 0o111,
        0,
        "with preserve_permissions off, the archived execute bit must not be applied"
    );
}
