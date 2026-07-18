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
