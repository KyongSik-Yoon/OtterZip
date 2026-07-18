//! C5 regression: a magic-detected single stream whose EXTENSION isn't the
//! canonical one (e.g. `logo.svgz` = gzip, or an extensionless `archive`)
//! must NOT derive an entry name equal to the source filename. If it did,
//! extracting into the source's own folder re-created the source path and
//! truncated the user's original file to 0 bytes (proven before the fix).

use std::fs;
use std::io::Write;

use flate2::write::GzEncoder;
use flate2::Compression;
use otterzip_core::{Archive, ExtractOptions, OpenMode, OverwritePolicy};

fn gzip_to(path: &std::path::Path, payload: &[u8]) {
    let f = fs::File::create(path).unwrap();
    let mut gz = GzEncoder::new(f, Compression::default());
    gz.write_all(payload).unwrap();
    gz.finish().unwrap();
}

#[test]
fn c5_svgz_never_truncates_its_source() {
    // The dangerous policy is Always (the CLI default). Rename was already
    // safe by luck; assert the source survives under BOTH.
    for policy in [OverwritePolicy::Always, OverwritePolicy::Rename] {
        let td = tempfile::tempdir().unwrap();
        let dir = td.path();
        let original = b"<svg>THE USERS ORIGINAL ARTWORK, IRREPLACEABLE</svg>";
        let src = dir.join("logo.svgz"); // gzip magic, non-.gz extension
        gzip_to(&src, original);
        let before = fs::read(&src).unwrap();

        let a = Archive::open(&src, OpenMode::Read).unwrap();
        let name = a.entries().unwrap().next().unwrap().unwrap().path.clone();

        let opts = ExtractOptions {
            destination: dir.to_path_buf(),
            overwrite: policy,
            ..Default::default()
        };
        let res = a.extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None);

        let after = fs::read(&src).unwrap();
        println!("policy {policy:?}: entry={name:?} src {}->{} extract={res:?}",
                 before.len(), after.len());

        assert_ne!(
            name, "logo.svgz",
            "entry name must not equal the source filename (would target the source)"
        );
        assert_eq!(
            after, before,
            "the source .svgz was mutated under {policy:?} — data loss"
        );
    }
}

#[test]
fn c5_extensionless_source_survives() {
    // The same trap hits an extensionless single stream (`archive`): the old
    // fallback returned the name verbatim.
    let td = tempfile::tempdir().unwrap();
    let dir = td.path();
    let src = dir.join("archive"); // no extension, gzip magic
    gzip_to(&src, b"payload bytes that must survive");
    let before = fs::read(&src).unwrap();

    let a = Archive::open(&src, OpenMode::Read).unwrap();
    let name = a.entries().unwrap().next().unwrap().unwrap().path.clone();
    let opts = ExtractOptions {
        destination: dir.to_path_buf(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let _ = a.extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None);

    assert_ne!(name, "archive", "extensionless entry name aliased the source");
    assert_eq!(fs::read(&src).unwrap(), before, "extensionless source mutated");
}

#[test]
fn c5_normal_gz_still_strips_cleanly() {
    // Guard against over-correction: a normal `.gz` must still strip to the
    // inner name, not gain a `.out` suffix.
    let td = tempfile::tempdir().unwrap();
    let dir = td.path();
    let src = dir.join("report.txt.gz");
    gzip_to(&src, b"hello");
    let a = Archive::open(&src, OpenMode::Read).unwrap();
    let name = a.entries().unwrap().next().unwrap().unwrap().path.clone();
    assert_eq!(name, "report.txt", "canonical .gz must strip to inner name");
}
