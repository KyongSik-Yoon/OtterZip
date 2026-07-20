//! Regression (claim 1): tar hard-link entries used to silently extract as
//! 0-byte files. They now materialise as a hard link to (or copy of) the
//! already-extracted target, mirroring its content.
//!
//! A GNU/BSD tar records the *second and subsequent* occurrences of a file
//! that shares an inode as `EntryType::Link` ('1') with size 0 and a
//! `linkname` pointing at the first occurrence. Every Linux rootfs tarball,
//! every `tar` of a tree with hardlinks (node_modules, .git alternates,
//! busybox applets) carries these.

use std::fs;
use std::io;

use otterzip_core::{Archive, ExtractOptions, OpenMode, OverwritePolicy, ProgressSink};
use tempfile::tempdir;

struct NullSink;
impl ProgressSink for NullSink {
    fn update(&mut self, _: &otterzip_core::Progress) -> bool {
        true
    }
}

#[test]

fn probe_tar_hardlink_entry() {
    let dir = tempdir().unwrap();
    let tar_path = dir.path().join("hardlink.tar");

    // Build: real.txt (12 bytes) + link.txt (hard link -> real.txt).
    {
        let file = fs::File::create(&tar_path).unwrap();
        let mut b = tar::Builder::new(file);

        let payload = b"REAL CONTENT";
        let mut h = tar::Header::new_gnu();
        h.set_path("real.txt").unwrap();
        h.set_size(payload.len() as u64);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        b.append(&h, &payload[..]).unwrap();

        let mut h2 = tar::Header::new_gnu();
        h2.set_path("link.txt").unwrap();
        h2.set_link_name("real.txt").unwrap();
        h2.set_size(0);
        h2.set_mode(0o644);
        h2.set_entry_type(tar::EntryType::Link);
        h2.set_cksum();
        b.append(&h2, io::empty()).unwrap();

        b.finish().unwrap();
    }

    let dest = dir.path().join("out");
    fs::create_dir_all(&dest).unwrap();

    let archive = Archive::open(&tar_path, OpenMode::Read).unwrap();

    println!("--- entries() ---");
    for e in archive.entries().unwrap() {
        let e = e.unwrap();
        println!(
            "  path={:?} size={} dir={} symlink={}",
            e.path, e.uncompressed_size, e.is_directory, e.is_symlink
        );
    }

    let opts = ExtractOptions {
        destination: dest.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let mut sink = NullSink;
    let report = archive.extract_all(&opts, Some(&mut sink)).unwrap();

    println!("--- report ---");
    println!("  entries_extracted = {}", report.entries_extracted);
    println!("  entries_skipped   = {}", report.entries_skipped);
    println!("  bytes_written     = {}", report.bytes_written);
    println!("  warnings          = {:?}", report.warnings);

    let real = dest.join("real.txt");
    let link = dest.join("link.txt");
    println!("--- filesystem ---");
    println!(
        "  real.txt exists={} len={:?}",
        real.exists(),
        fs::metadata(&real).map(|m| m.len()).ok()
    );
    println!(
        "  link.txt exists={} len={:?} content={:?}",
        link.exists(),
        fs::metadata(&link).map(|m| m.len()).ok(),
        fs::read(&link).ok()
    );

    // What a user expects: link.txt has the same content as real.txt.
    assert!(link.exists(), "link.txt was not created at all");
    assert_eq!(
        fs::read(&link).unwrap(),
        b"REAL CONTENT",
        "REPRODUCED: hard-link entry extracted with the wrong content \
         (should mirror its link target)"
    );
}

/// Security: a hard-link whose target escapes dest_root must NOT be turned into
/// an alias for an outside file. The link name is attacker-controlled, so it is
/// routed through the same traversal guard as real entry paths.
#[test]
fn hardlink_target_escaping_dest_root_is_blocked() {
    let dir = tempdir().unwrap();

    // A secret living OUTSIDE the extraction destination.
    let secret = dir.path().join("secret.txt");
    fs::write(&secret, b"TOP SECRET").unwrap();

    let tar_path = dir.path().join("evil.tar");
    {
        let file = fs::File::create(&tar_path).unwrap();
        let mut b = tar::Builder::new(file);
        // A hard link whose target climbs out of dest_root to the secret.
        let mut h = tar::Header::new_gnu();
        h.set_path("pwned.txt").unwrap();
        h.set_link_name("../secret.txt").unwrap();
        h.set_size(0);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Link);
        h.set_cksum();
        b.append(&h, io::empty()).unwrap();
        b.finish().unwrap();
    }

    let dest = dir.path().join("out");
    fs::create_dir_all(&dest).unwrap();
    let opts = ExtractOptions {
        destination: dest.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default() // block_path_traversal: true
    };
    let mut sink = NullSink;
    let result = archive_extract(&tar_path, &opts, &mut sink);

    // Either the run errors with a traversal block, or the entry is skipped —
    // but under no circumstances may an alias to the outside secret appear.
    assert!(
        !dest.join("pwned.txt").exists(),
        "SECURITY: a hard-link with a ../ target aliased a file outside dest_root"
    );
    if let Ok(report) = result {
        assert_eq!(report.entries_extracted, 0, "the escaping link must not extract");
    }
}

fn archive_extract(
    path: &std::path::Path,
    opts: &ExtractOptions,
    sink: &mut NullSink,
) -> Result<otterzip_core::ExtractReport, otterzip_core::OtterzipError> {
    Archive::open(path, OpenMode::Read)
        .unwrap()
        .extract_all(opts, Some(sink))
}
