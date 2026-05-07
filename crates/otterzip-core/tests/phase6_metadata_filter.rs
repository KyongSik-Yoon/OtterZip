//! Phase 6+ — `CreateOptions::exclude_system_metadata` filter.
//!
//! Build a tiny on-disk tree containing both real files and the OS-noise
//! files we want filtered out, then assert the resulting ZIP has only the
//! expected entry set when the toggle is on / off.

use std::fs;
use std::path::PathBuf;

use otterzip_core::format::ArchiveFormat;
use otterzip_core::options::{is_system_metadata, CreateOptions};
use otterzip_core::{Archive, OpenMode};

fn temp_dir(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "otterzip-phase6-{}-{}",
        name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn write(p: &PathBuf, name: &str, content: &str) {
    let full = p.join(name);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(full, content).unwrap();
}

fn list_entry_names(zip_path: &PathBuf) -> Vec<String> {
    let archive = Archive::open(zip_path, OpenMode::Read).unwrap();
    let mut names: Vec<String> = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path)
        .collect();
    names.sort();
    names
}

#[test]
fn exclude_filter_drops_thumbs_db_and_ds_store() {
    let src = temp_dir("exclude-on-src");
    write(&src, "real.txt", "hello");
    write(&src, "Thumbs.db", "noise");
    write(&src, ".DS_Store", "noise");
    write(&src, "sub/photo.jpg", "img");
    write(&src, "sub/desktop.ini", "noise");

    let dest = temp_dir("exclude-on-dest").join("out.zip");
    let opts = CreateOptions {
        format: ArchiveFormat::Zip,
        exclude_system_metadata: true,
        ..CreateOptions::default()
    };
    let mut archive = Archive::create(&dest, opts).unwrap();
    archive
        .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&src, "", None)
        .unwrap();
    archive.commit().unwrap();

    let names = list_entry_names(&dest);
    assert!(names.iter().any(|n| n == "real.txt"));
    assert!(names.iter().any(|n| n == "sub/photo.jpg"));
    assert!(!names.iter().any(|n| n.ends_with("Thumbs.db")));
    assert!(!names.iter().any(|n| n.ends_with(".DS_Store")));
    assert!(!names.iter().any(|n| n.ends_with("desktop.ini")));
}

#[test]
fn exclude_filter_off_keeps_everything() {
    let src = temp_dir("exclude-off-src");
    write(&src, "real.txt", "hello");
    write(&src, "Thumbs.db", "noise");
    write(&src, ".DS_Store", "noise");

    let dest = temp_dir("exclude-off-dest").join("out.zip");
    let opts = CreateOptions {
        format: ArchiveFormat::Zip,
        exclude_system_metadata: false,
        ..CreateOptions::default()
    };
    let mut archive = Archive::create(&dest, opts).unwrap();
    archive
        .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&src, "", None)
        .unwrap();
    archive.commit().unwrap();

    let names = list_entry_names(&dest);
    assert!(names.iter().any(|n| n.ends_with("Thumbs.db")));
    assert!(names.iter().any(|n| n.ends_with(".DS_Store")));
}

#[test]
fn is_system_metadata_basenames() {
    use std::path::Path;
    assert!(is_system_metadata(Path::new("a/b/Thumbs.db")));
    assert!(is_system_metadata(Path::new("a/.DS_Store")));
    assert!(is_system_metadata(Path::new("desktop.ini")));
    assert!(is_system_metadata(Path::new("ehthumbs.db")));
    // Case-insensitive (ASCII).
    assert!(is_system_metadata(Path::new("THUMBS.DB")));
    // AppleDouble shadow.
    assert!(is_system_metadata(Path::new("dir/._photo.jpg")));
    // Folder match anywhere on the path.
    assert!(is_system_metadata(Path::new("vol/$RECYCLE.BIN/inside.txt")));
    assert!(is_system_metadata(Path::new("vol/System Volume Information/x")));
    // Innocent files survive.
    assert!(!is_system_metadata(Path::new("photo.jpg")));
    assert!(!is_system_metadata(Path::new("readme.md")));
    // Underscore-prefix that isn't AppleDouble: only "._" is special.
    assert!(!is_system_metadata(Path::new("_real_file.txt")));
}
