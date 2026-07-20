//! T1-4 (create side): `CreateOptions::preserve_timestamps` (default true) must
//! carry the SOURCE file's mtime into each entry — via add_file AND the parallel
//! add_dir_recursive bulk pipeline. Regression guard for the create-side wire;
//! the field used to be inert, stamping every ZIP entry with wall-clock now (a
//! silent data loss: a freshly-made archive already lost the original dates).

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use otterzip_core::{format::CompressionMethod, Archive, ArchiveFormat, CreateOptions, OpenMode};
use tempfile::tempdir;

/// 1990-06-15 12:00:00 UTC.
const OLD_UNIX_SECS: u64 = 645_451_200;

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn write_old_file(p: &Path, body: &[u8]) {
    fs::write(p, body).unwrap();
    let old = UNIX_EPOCH + Duration::from_secs(OLD_UNIX_SECS);
    let f = fs::File::options().write(true).open(p).unwrap();
    let times = fs::FileTimes::new().set_accessed(old).set_modified(old);
    f.set_times(times).unwrap();
    drop(f);
    let back = secs(fs::metadata(p).unwrap().modified().unwrap());
    println!("source file on disk has mtime {back} unix secs (wanted {OLD_UNIX_SECS})");
    assert_eq!(back, OLD_UNIX_SECS, "fixture sanity: set_times must stick");
}

fn archived_mtime(zip_path: &Path, name: &str) -> Option<u64> {
    let a = Archive::open(zip_path, OpenMode::Read).unwrap();
    let found = a
        .entries()
        .unwrap()
        .filter_map(std::result::Result::ok)
        .find(|e| e.path == name)
        .and_then(|e| e.modified)
        .map(secs);
    found
}

#[test]
fn create_stamps_entries_with_the_source_file_mtime() {
    let td = tempdir().unwrap();
    let src = td.path().join("vintage.txt");
    write_old_file(&src, b"written in 1990\n");

    let dest = td.path().join("out.zip");
    let mut archive = Archive::create(
        &dest,
        CreateOptions {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            preserve_timestamps: true,
            ..Default::default()
        },
    )
    .unwrap();
    archive.add_file(&src, "vintage.txt").unwrap();
    archive.commit().unwrap();

    let got = archived_mtime(&dest, "vintage.txt").unwrap();
    let now = secs(SystemTime::now());
    println!(
        "preserve_timestamps=true -> entry mtime in ZIP = {got} unix secs \
         (source {OLD_UNIX_SECS}, now {now}, off-by {} s)",
        now.saturating_sub(got)
    );

    assert_eq!(
        got, OLD_UNIX_SECS,
        "preserve_timestamps=true but the ZIP entry was stamped with wall-clock now"
    );
}

#[test]
fn create_timestamp_flag_changes_behaviour_at_all() {
    let td = tempdir().unwrap();
    let src = td.path().join("vintage.txt");
    write_old_file(&src, b"written in 1990\n");

    let mut stamps = Vec::new();
    for keep in [true, false] {
        let dest = td.path().join(format!("out-{keep}.zip"));
        let mut archive = Archive::create(
            &dest,
            CreateOptions {
                format: ArchiveFormat::Zip,
                preserve_timestamps: keep,
                ..Default::default()
            },
        )
        .unwrap();
        archive.add_file(&src, "vintage.txt").unwrap();
        archive.commit().unwrap();
        let m = archived_mtime(&dest, "vintage.txt").unwrap();
        println!("preserve_timestamps={keep} -> entry mtime {m}");
        stamps.push(m);
    }
    assert_ne!(
        stamps[0], stamps[1],
        "preserve_timestamps true/false produced the same entry stamp — flag is inert"
    );
}

#[test]
fn create_via_add_dir_recursive_also_preserves() {
    // The GUI's normal path is add_dir_recursive (parallel bulk pipeline),
    // which is a different code path from add_file. Check it too.
    let td = tempdir().unwrap();
    let srcdir = td.path().join("tree");
    fs::create_dir_all(&srcdir).unwrap();
    let f = srcdir.join("vintage.txt");
    write_old_file(&f, b"written in 1990\n");

    let dest = td.path().join("tree.zip");
    let mut archive = Archive::create(
        &dest,
        CreateOptions {
            format: ArchiveFormat::Zip,
            preserve_timestamps: true,
            ..Default::default()
        },
    )
    .unwrap();
    archive
        .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&srcdir, "", None)
        .unwrap();
    archive.commit().unwrap();

    let got = archived_mtime(&dest, "vintage.txt").unwrap();
    println!("add_dir_recursive -> entry mtime {got} (source {OLD_UNIX_SECS})");
    assert_eq!(
        got, OLD_UNIX_SECS,
        "add_dir_recursive also drops the source mtime"
    );
}

#[test]
fn create_via_parallel_bulk_pipeline_preserves() {
    // The single-file add_dir_recursive test above stays under
    // PARALLEL_MIN_ENTRIES (16) and falls back to the serial walker. Push past
    // the threshold so the rayon `prepare_entry` bulk path is the one under
    // test — that path derives each entry's DOS stamp from the file's own mtime
    // rather than the batch-wide "now" fallback.
    let td = tempdir().unwrap();
    let srcdir = td.path().join("many");
    fs::create_dir_all(&srcdir).unwrap();
    for i in 0..24 {
        write_old_file(&srcdir.join(format!("f{i:02}.txt")), b"vintage bulk\n");
    }

    let dest = td.path().join("many.zip");
    let mut archive = Archive::create(
        &dest,
        CreateOptions {
            format: ArchiveFormat::Zip,
            preserve_timestamps: true,
            ..Default::default()
        },
    )
    .unwrap();
    archive
        .add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(&srcdir, "", None)
        .unwrap();
    archive.commit().unwrap();

    for i in 0..24 {
        let name = format!("f{i:02}.txt");
        let got = archived_mtime(&dest, &name).unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(
            got, OLD_UNIX_SECS,
            "parallel bulk entry {name} lost the source mtime (got {got})"
        );
    }
}

#[test]
fn create_tar_gz_preserves_source_mtime() {
    // tar carries mtime natively; `Header::new_gnu` defaults it to 0 (1970),
    // so without the wire a created tar loses every date. Round-trip check.
    let td = tempdir().unwrap();
    let src = td.path().join("vintage.txt");
    write_old_file(&src, b"tar vintage\n");

    let dest = td.path().join("out.tar.gz");
    let mut archive = Archive::create(
        &dest,
        CreateOptions {
            format: ArchiveFormat::TarGz,
            preserve_timestamps: true,
            ..Default::default()
        },
    )
    .unwrap();
    archive.add_file(&src, "vintage.txt").unwrap();
    archive.commit().unwrap();

    let got = archived_mtime(&dest, "vintage.txt").unwrap();
    println!("tar.gz create -> entry mtime {got} (source {OLD_UNIX_SECS})");
    assert_eq!(got, OLD_UNIX_SECS, "tar.gz create dropped the source mtime");
}

#[test]
fn create_7z_preserves_source_mtime() {
    // 7z stores an NtTime per entry; the writer used ArchiveEntry::default()
    // (no timestamp) until the wire set last_modified_date. Round-trip check.
    let td = tempdir().unwrap();
    let src = td.path().join("vintage.txt");
    write_old_file(&src, b"7z vintage\n");

    let dest = td.path().join("out.7z");
    let mut archive = Archive::create(
        &dest,
        CreateOptions {
            format: ArchiveFormat::SevenZ,
            preserve_timestamps: true,
            ..Default::default()
        },
    )
    .unwrap();
    archive.add_file(&src, "vintage.txt").unwrap();
    archive.commit().unwrap();

    let got = archived_mtime(&dest, "vintage.txt").unwrap();
    println!("7z create -> entry mtime {got} (source {OLD_UNIX_SECS})");
    assert_eq!(got, OLD_UNIX_SECS, "7z create dropped the source mtime");
}
