//! Extraction path-safety regressions.
//!
//! C1: an entry named "" / "." produces zero path components, so the resolver
//! never inspected a component and returned dest_root itself; under the GUI's
//! default keep-both policy the writer then walked UP to the parent and wrote
//! `<dest> (2)` OUTSIDE the destination. The CLI only uses Always/IfNewer/Never
//! so it can't reach this, but ExtractDefaults.cs makes Rename the app default.
//! Fixed by rejecting `out == dest_root` in the one shared resolver, which the
//! serial, parallel and lenient paths now all route through.

use std::fs;
use std::io::Write;
use std::path::Path;

use otterzip_core::{Archive, ExtractOptions, OpenMode, OverwritePolicy};

fn build_zip_with_name(path: &Path, name: &str, payload: &[u8]) {
    let f = fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let o =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    z.start_file(name, o).unwrap();
    z.write_all(payload).unwrap();
    z.finish().unwrap();
}

#[test]
fn c1_empty_entry_name_under_rename_policy() {
    for entry_name in ["", "."] {
        let td = tempfile::tempdir().unwrap();
        let base = td.path();
        let arc = base.join("evil.zip");
        build_zip_with_name(&arc, entry_name, b"ESCAPED-THE-DEST-ROOT\n");

        let dest = base.join("out");
        fs::create_dir_all(&dest).unwrap();

        let a = Archive::open(&arc, OpenMode::Read).unwrap();
        let opts = ExtractOptions {
            destination: dest.clone(),
            overwrite: OverwritePolicy::Rename, // the app's shipping default
            block_path_traversal: true,
            ..Default::default()
        };

        let res = a.extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None);

        let mut outside = vec![];
        for e in fs::read_dir(base).unwrap() {
            let p = e.unwrap().path();
            if p == arc || p == dest {
                continue;
            }
            outside.push(p.file_name().unwrap().to_string_lossy().to_string());
        }

        println!("--- entry {entry_name:?} ---");
        println!("  extract_all -> {res:?}");
        println!("  siblings of dest (must be EMPTY): {outside:?}");
        for name in &outside {
            let p = base.join(name);
            if p.is_file() {
                println!(
                    "    {name} = {:?}",
                    String::from_utf8_lossy(&fs::read(&p).unwrap())
                );
            }
        }
        assert!(
            outside.is_empty(),
            "entry {entry_name:?} wrote OUTSIDE dest_root: {outside:?}"
        );
    }
}


// ---------------------------------------------------------------------------
// The other half of the rule above: a DIRECTORY that resolves to dest_root is
// the destination itself, not an escape. `./` is the first member of every
// `tar -cf x.tar -C dir .` — the most common tar idiom there is. Rejecting it
// (as the first cut of the C1 fix did) made those archives extract to NOTHING:
//   extract_all -> Err(PathTraversalBlocked("./")),  files on disk: []
// So the guard keys on the entry kind: directory -> Ok(dest_root), file -> reject.
// ---------------------------------------------------------------------------

#[test]
fn dot_slash_root_directory_entry_still_extracts() {
    let td = tempfile::tempdir().unwrap();
    let dir = td.path();

    // Build a tar exactly the way `tar -C dir .` does: a "./" directory member
    // first, then "./file.txt".
    let tar_path = dir.join("dot.tar");
    {
        let f = fs::File::create(&tar_path).unwrap();
        let mut b = tar::Builder::new(f);

        // "./" directory member
        let mut h = tar::Header::new_gnu();
        h.set_path("./").unwrap();
        h.set_size(0);
        h.set_entry_type(tar::EntryType::Directory);
        h.set_mode(0o755);
        h.set_cksum();
        b.append(&h, std::io::empty()).unwrap();

        // "./file.txt"
        let body = b"hello from a dot-slash tar\n";
        let mut h2 = tar::Header::new_gnu();
        h2.set_path("./file.txt").unwrap();
        h2.set_size(body.len() as u64);
        h2.set_entry_type(tar::EntryType::Regular);
        h2.set_mode(0o644);
        h2.set_cksum();
        b.append(&h2, &body[..]).unwrap();

        // "./sub/nested.txt"
        let body2 = b"nested\n";
        let mut h3 = tar::Header::new_gnu();
        h3.set_path("./sub/nested.txt").unwrap();
        h3.set_size(body2.len() as u64);
        h3.set_entry_type(tar::EntryType::Regular);
        h3.set_mode(0o644);
        h3.set_cksum();
        b.append(&h3, &body2[..]).unwrap();

        b.finish().unwrap();
    }
    let mut f = fs::File::create(dir.join("_keep")).unwrap();
    f.write_all(b"x").unwrap();

    let a = Archive::open(&tar_path, OpenMode::Read).unwrap();
    let names: Vec<String> = a
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path.clone())
        .collect();
    println!("entries in archive: {names:?}");

    let dest = dir.join("out");
    fs::create_dir_all(&dest).unwrap();
    let opts = ExtractOptions {
        destination: dest.clone(),
        overwrite: OverwritePolicy::Always,
        ..Default::default()
    };
    let res = a.extract_all::<fn(&otterzip_core::Progress) -> bool>(&opts, None);
    println!("extract_all -> {res:?}");

    let mut landed = vec![];
    for e in walkdir(&dest) {
        landed.push(e);
    }
    println!("files on disk: {landed:?}");

    assert!(res.is_ok(), "a normal `tar -C dir .` archive failed: {res:?}");
    assert!(
        landed.iter().any(|p| p.ends_with("file.txt")),
        "file.txt missing; got {landed:?}"
    );
    assert!(
        landed.iter().any(|p| p.ends_with("nested.txt")),
        "sub/nested.txt missing; got {landed:?}"
    );
}

fn walkdir(root: &std::path::Path) -> Vec<String> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(
                        p.strip_prefix(root)
                            .unwrap_or(&p)
                            .to_string_lossy()
                            .to_string(),
                    );
                }
            }
        }
    }
    out
}
