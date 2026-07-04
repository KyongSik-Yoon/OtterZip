//! Sprint 4 integration tests: archive *creation*. Round-trips a small
//! file tree through `Archive::create` + `add_file` + `commit`, then
//! re-opens via `Archive::open` and verifies content.

use std::fs;
use std::io::Write;

use otterzip_core::{
    Archive, ArchiveFormat, CreateOptions, ExtractOptions, OpenMode, OverwritePolicy,
};
use otterzip_core::format::{CompressionMethod, EncryptionMethod};
use tempfile::tempdir;
use zeroize::Zeroizing;

fn write_source_tree(root: &std::path::Path) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("a.txt"), b"alpha\n").unwrap();
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("nested/b.bin"), &[1u8, 2, 3, 4, 5]).unwrap();
}

fn extract_and_verify(archive_path: &std::path::Path, td: &std::path::Path) {
    let out = td.join("out");
    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let archive = Archive::open(archive_path, OpenMode::Read).unwrap();
    let report = archive
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert!(report.entries_extracted >= 2);
    assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"alpha\n");
    assert_eq!(
        fs::read(out.join("nested").join("b.bin")).unwrap(),
        [1u8, 2, 3, 4, 5]
    );
}

#[test]
fn create_zip_then_extract_roundtrip() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let zip_path = td.path().join("out.zip");

    {
        let opts = CreateOptions {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            ..Default::default()
        };
        let mut archive = Archive::create(&zip_path, opts).unwrap();
        archive.add_file(src.join("a.txt"), "a.txt").unwrap();
        archive
            .add_file(src.join("nested/b.bin"), "nested/b.bin")
            .unwrap();
        archive.commit().unwrap();
    }

    extract_and_verify(&zip_path, td.path());
}

#[test]
fn extract_rename_policy_keeps_both() {
    // OverwritePolicy::Rename (keep-both): a colliding output file is
    // diverted to `name (2).ext` instead of overwriting.
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src); // a.txt = "alpha\n"
    let zip_path = td.path().join("out.zip");
    {
        let opts = CreateOptions {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            ..Default::default()
        };
        let mut archive = Archive::create(&zip_path, opts).unwrap();
        archive.add_file(src.join("a.txt"), "a.txt").unwrap();
        archive.commit().unwrap();
    }

    let out = td.path().join("dest");
    fs::create_dir_all(&out).unwrap();
    fs::write(out.join("a.txt"), b"existing\n").unwrap(); // pre-existing collision

    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Rename,
        ..Default::default()
    };
    let archive = Archive::open(&zip_path, OpenMode::Read).unwrap();
    archive
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();

    // Original untouched; archive content lands in a renamed sibling.
    assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"existing\n");
    assert_eq!(fs::read(out.join("a (2).txt")).unwrap(), b"alpha\n");
}

#[test]
fn create_7z_aes256_roundtrip() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src); // a.txt = "alpha\n"
    let archive = td.path().join("enc.7z");
    {
        let opts = CreateOptions {
            format: ArchiveFormat::SevenZ,
            compression: CompressionMethod::Lzma2,
            compression_level: 5,
            encryption: EncryptionMethod::Aes256,
            password: Some(Zeroizing::new("s3cret".to_string())),
            ..Default::default()
        };
        let mut archive_w = Archive::create(&archive, opts).unwrap();
        archive_w.add_file(src.join("a.txt"), "a.txt").unwrap();
        archive_w.commit().unwrap();
    }

    let out = td.path().join("out");
    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let reader = Archive::open_with_password(&archive, OpenMode::Read, "s3cret".to_string()).unwrap();
    reader
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"alpha\n");
}

#[test]
fn create_zip_aes256_roundtrip() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let archive = td.path().join("enc.zip");
    {
        let opts = CreateOptions {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            encryption: EncryptionMethod::Aes256,
            password: Some(Zeroizing::new("s3cret".to_string())),
            ..Default::default()
        };
        let mut archive_w = Archive::create(&archive, opts).unwrap();
        archive_w.add_file(src.join("a.txt"), "a.txt").unwrap();
        archive_w.commit().unwrap();
    }

    // The archive should report as encrypted.
    let probe = Archive::open(&archive, OpenMode::Read).unwrap();
    assert!(probe.is_encrypted().unwrap(), "AES zip must report encrypted");

    let out = td.path().join("out");
    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let reader = Archive::open_with_password(&archive, OpenMode::Read, "s3cret".to_string()).unwrap();
    reader
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"alpha\n");
}

#[test]
fn create_password_without_method_fails_closed() {
    // SECURITY regression guard. Supplying a password while leaving
    // `encryption` at its default (None) must NOT silently write a PLAINTEXT
    // archive — `Archive::create` upgrades password+None to AES-256. The
    // result must therefore report encrypted and round-trip via the password.
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let archive = td.path().join("guard.zip");
    {
        let opts = CreateOptions {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            // `encryption` deliberately left at the default (None) while a
            // password IS supplied — the fail-closed path under test.
            password: Some(Zeroizing::new("s3cret".to_string())),
            ..Default::default()
        };
        let mut archive_w = Archive::create(&archive, opts).unwrap();
        archive_w.add_file(src.join("a.txt"), "a.txt").unwrap();
        archive_w.commit().unwrap();
    }

    // Must be encrypted despite encryption=None at create time.
    let probe = Archive::open(&archive, OpenMode::Read).unwrap();
    assert!(
        probe.is_encrypted().unwrap(),
        "password without an explicit method must still encrypt (fail-closed)"
    );

    // And it must round-trip with the supplied password.
    let out = td.path().join("out");
    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let reader =
        Archive::open_with_password(&archive, OpenMode::Read, "s3cret".to_string()).unwrap();
    reader
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"alpha\n");
}

#[test]
fn create_7z_solid_roundtrip() {
    // Solid 7z packs small entries into shared blocks; verify the result
    // is still a valid, fully-extractable archive.
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let archive = td.path().join("solid.7z");
    {
        let opts = CreateOptions {
            format: ArchiveFormat::SevenZ,
            compression: CompressionMethod::Lzma2,
            compression_level: 5,
            solid: true,
            ..Default::default()
        };
        let mut archive_w = Archive::create(&archive, opts).unwrap();
        archive_w
            .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&src, "", None)
            .unwrap();
        archive_w.commit().unwrap();
    }

    let out = td.path().join("out");
    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let reader = Archive::open(&archive, OpenMode::Read).unwrap();
    reader
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(fs::read(out.join("a.txt")).unwrap(), b"alpha\n");
    assert_eq!(
        fs::read(out.join("nested").join("b.bin")).unwrap(),
        [1u8, 2, 3, 4, 5]
    );
}

#[test]
fn create_zip_split_volumes_roundtrip() {
    // Split (volume) creation: a contiguous archive is sliced into
    // split.zip.001/.002/... segments. Re-concatenating them must
    // reproduce a byte-identical, fully-extractable archive (the read
    // side / 7-Zip / Bandizip treat the `.NNN` set as raw split).
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    fs::create_dir_all(&src).unwrap();
    // Incompressible payload (LCG) so the segments actually span several
    // volumes regardless of the codec.
    let mut blob = vec![0u8; 48 * 1024];
    let mut state: u32 = 0x1234_5678;
    for b in blob.iter_mut() {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        *b = (state >> 16) as u8;
    }
    fs::write(src.join("blob.bin"), &blob).unwrap();

    let archive = td.path().join("split.zip");
    let volume = 16 * 1024u64;
    {
        let opts = CreateOptions {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            volume_size_bytes: Some(volume),
            ..Default::default()
        };
        let mut w = Archive::create(&archive, opts).unwrap();
        w.add_file(src.join("blob.bin"), "blob.bin").unwrap();
        w.commit().unwrap();
    }

    // Split mode must leave neither an unsuffixed archive nor the temp.
    assert!(
        !archive.exists(),
        "split mode must not leave an unsuffixed archive"
    );
    assert!(
        !td.path().join("split.zip.otzpart").exists(),
        "temp must be removed after commit"
    );

    // Collect split.zip.001, .002, ...
    let mut segments = Vec::new();
    for idx in 1u32..=999 {
        let seg = td.path().join(format!("split.zip.{idx:03}"));
        if !seg.exists() {
            break;
        }
        segments.push(seg);
    }
    assert!(
        segments.len() >= 2,
        "expected >=2 segments, got {}",
        segments.len()
    );
    // Every non-final segment is exactly one volume in size.
    for seg in &segments[..segments.len() - 1] {
        assert_eq!(fs::metadata(seg).unwrap().len(), volume);
    }

    // Re-concatenate -> single archive -> open -> extract -> verify.
    let joined = td.path().join("rejoined.zip");
    {
        let mut out = fs::File::create(&joined).unwrap();
        for seg in &segments {
            out.write_all(&fs::read(seg).unwrap()).unwrap();
        }
        out.flush().unwrap();
    }

    let out = td.path().join("out");
    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let reader = Archive::open(&joined, OpenMode::Read).unwrap();
    reader
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(fs::read(out.join("blob.bin")).unwrap(), blob);
}

#[test]
fn create_7z_aes_split_roundtrip() {
    // Combined: AES-256 + volume split. Split is a post-write byte slice,
    // so it must compose with encryption — concatenating the encrypted
    // segments reproduces a password-openable archive.
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let mut blob = vec![0u8; 40 * 1024];
    let mut state: u32 = 0x0BADF00D;
    for b in blob.iter_mut() {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        *b = (state >> 16) as u8;
    }
    fs::write(src.join("blob.bin"), &blob).unwrap();

    let archive = td.path().join("enc-split.7z");
    let volume = 16 * 1024u64;
    {
        let opts = CreateOptions {
            format: ArchiveFormat::SevenZ,
            compression: CompressionMethod::Lzma2,
            compression_level: 5,
            encryption: EncryptionMethod::Aes256,
            password: Some(Zeroizing::new("s3cret".to_string())),
            volume_size_bytes: Some(volume),
            ..Default::default()
        };
        let mut w = Archive::create(&archive, opts).unwrap();
        w.add_file(src.join("blob.bin"), "blob.bin").unwrap();
        w.commit().unwrap();
    }

    let mut segments = Vec::new();
    for idx in 1u32..=999 {
        let seg = td.path().join(format!("enc-split.7z.{idx:03}"));
        if !seg.exists() {
            break;
        }
        segments.push(seg);
    }
    assert!(segments.len() >= 2, "expected >=2 segments, got {}", segments.len());

    let joined = td.path().join("rejoined.7z");
    {
        let mut out = fs::File::create(&joined).unwrap();
        for seg in &segments {
            out.write_all(&fs::read(seg).unwrap()).unwrap();
        }
        out.flush().unwrap();
    }

    let out = td.path().join("out");
    let opts = ExtractOptions {
        destination: out.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let reader = Archive::open_with_password(&joined, OpenMode::Read, "s3cret".to_string()).unwrap();
    reader
        .extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None)
        .unwrap();
    assert_eq!(fs::read(out.join("blob.bin")).unwrap(), blob);
}

#[test]
fn create_zip_with_add_dir_recursive() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let zip_path = td.path().join("out.zip");

    {
        let opts = CreateOptions {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            ..Default::default()
        };
        let mut archive = Archive::create(&zip_path, opts).unwrap();
        archive
            .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&src, "", None)
            .unwrap();
        archive.commit().unwrap();
    }

    extract_and_verify(&zip_path, td.path());
}

#[test]
fn create_7z_then_extract_roundtrip() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let archive_path = td.path().join("out.7z");

    {
        let opts = CreateOptions {
            format: ArchiveFormat::SevenZ,
            compression: CompressionMethod::Lzma2,
            compression_level: 5,
            ..Default::default()
        };
        let mut archive = Archive::create(&archive_path, opts).unwrap();
        archive
            .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&src, "", None)
            .unwrap();
        archive.commit().unwrap();
    }

    extract_and_verify(&archive_path, td.path());
}

#[test]
fn create_targz_then_extract_roundtrip() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let archive_path = td.path().join("out.tar.gz");

    {
        let opts = CreateOptions {
            format: ArchiveFormat::TarGz,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            ..Default::default()
        };
        let mut archive = Archive::create(&archive_path, opts).unwrap();
        archive
            .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&src, "", None)
            .unwrap();
        archive.commit().unwrap();
    }

    extract_and_verify(&archive_path, td.path());
}

#[test]
fn rar_creation_is_explicitly_blocked() {
    let td = tempdir().unwrap();
    let opts = CreateOptions {
        format: ArchiveFormat::Rar,
        compression: CompressionMethod::Lzma2,
        compression_level: 5,
        ..Default::default()
    };
    let err = Archive::create(td.path().join("out.rar"), opts).unwrap_err();
    assert!(matches!(
        err,
        otterzip_core::OtterzipError::FeatureDisabled(_)
    ));
}

#[test]
fn rollback_removes_incomplete_archive() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let zip_path = td.path().join("incomplete.zip");

    let opts = CreateOptions {
        format: ArchiveFormat::Zip,
        compression: CompressionMethod::Deflate,
        compression_level: 5,
        ..Default::default()
    };
    let mut archive = Archive::create(&zip_path, opts).unwrap();
    archive.add_file(src.join("a.txt"), "a.txt").unwrap();
    archive.rollback().unwrap();

    assert!(!zip_path.exists(), "rollback should remove the file");
}

#[test]
fn add_file_on_read_mode_archive_errors() {
    let td = tempdir().unwrap();
    let src = td.path().join("src");
    write_source_tree(&src);
    let zip_path = td.path().join("seed.zip");
    {
        let opts = CreateOptions {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            compression_level: 5,
            ..Default::default()
        };
        let mut archive = Archive::create(&zip_path, opts).unwrap();
        archive.add_file(src.join("a.txt"), "a.txt").unwrap();
        archive.commit().unwrap();
    }

    let mut read = Archive::open(&zip_path, OpenMode::Read).unwrap();
    let err = read
        .add_file(src.join("a.txt"), "another.txt")
        .unwrap_err();
    assert!(matches!(
        err,
        otterzip_core::OtterzipError::InvalidArgument(_)
    ));
}

/// Regression: creating a password-protected ZIP must produce an archive
/// that is ACTUALLY AES-256 encrypted on disk — not silently plaintext.
///
/// The prior `sprint3_backends::encrypted_zip_unlocks_with_correct_password`
/// only exercised the READ path against a fixture built by the `zip` crate;
/// nothing asserted that OtterZip's own writer encrypts. A stale-binary
/// scare during the 1.0.1 smoke test showed how invisible a silent-plaintext
/// regression would be, so this pins the create side:
///   - the on-disk bytes carry the AES extra-field marker (0x9901),
///   - the plaintext never appears in the archive,
///   - opening WITHOUT the password fails to read the entry, and
///   - `test()` reports corruption without the password but passes with it
///     (the exact contract the app's verify-after-compress relies on).
#[test]
fn create_encrypted_zip_is_actually_encrypted() {
    let td = tempdir().unwrap();
    let src = td.path().join("secret.txt");
    let secret = b"TOP-SECRET-PLAINTEXT-must-not-appear-in-the-archive";
    fs::write(&src, secret).unwrap();
    let zip_path = td.path().join("enc.zip");

    let opts = CreateOptions {
        format: ArchiveFormat::Zip,
        password: Some(Zeroizing::new("correct horse".to_string())),
        encryption: EncryptionMethod::Aes256,
        ..Default::default()
    };
    let mut archive = Archive::create(&zip_path, opts).unwrap();
    archive.add_file(&src, "secret.txt").unwrap();
    archive.commit().unwrap();

    // On-disk format checks: AES marker present, plaintext absent.
    let bytes = fs::read(&zip_path).unwrap();
    assert!(
        bytes.windows(2).any(|w| w == [0x01, 0x99]),
        "AES-256 extra field (0x9901) missing — ZIP was written unencrypted"
    );
    assert!(
        !bytes
            .windows(secret.len())
            .any(|w| w == secret),
        "plaintext secret leaked into the archive — not encrypted"
    );

    // Verify contract: test() fails without the password, passes with it.
    let no_pw = Archive::open(&zip_path, OpenMode::Read).unwrap();
    let bad = no_pw
        .test::<fn(&otterzip_core::Progress) -> bool>(None)
        .unwrap();
    assert!(
        bad.entries_corrupted >= 1,
        "test() without password must report the encrypted entry as unreadable"
    );

    let with_pw =
        Archive::open_with_password(&zip_path, OpenMode::Read, "correct horse".to_owned())
            .unwrap();
    let good = with_pw
        .test::<fn(&otterzip_core::Progress) -> bool>(None)
        .unwrap();
    assert_eq!(
        good.entries_corrupted, 0,
        "test() with the correct password must pass"
    );
    assert_eq!(good.entries_tested, 1);
}
