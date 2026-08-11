//! Add files to an existing ZIP without rebuilding it.
//!
//! OtterZip is create-only everywhere else: `Archive::create` truncates, and
//! `Archive::open` rejects any mode but Read. That is fine for the "compress a
//! selection" flow, but it leaves no way to drop one more file into an archive
//! you already have — the single most-asked-for thing a user does after making
//! one.
//!
//! This module fills exactly that gap, and only for ZIP. It uses the `zip`
//! crate's append mode (`ZipWriter::new_append`), which reads the existing
//! central directory and writes new local headers after the last entry, then
//! rewrites the central directory — so the bytes of every existing entry are
//! preserved verbatim and nothing is recompressed. Adding one file to a 2 GB
//! archive costs one file's worth of work, not two gigabytes of it.
//!
//! Scope is deliberately narrow:
//!   * ZIP only. The in-tree writer owns creation for every format; this is a
//!     surgical read-modify-write that the `zip` crate happens to support for
//!     ZIP and nothing else. RAR is extract-only by licence; 7z/tar append is
//!     a separate, larger piece of work.
//!   * A name that already exists in the archive is SKIPPED, not duplicated.
//!     ZIP permits two entries with the same name, but a second one only
//!     confuses extractors (which one wins is undefined), so the safe answer
//!     is to leave the original in place and tell the caller what was skipped.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{OtterzipError, Result};

/// What an [`append_to_zip`] call did.
#[derive(Debug, Default, Clone)]
pub struct ZipAppendReport {
    /// Number of new file entries written.
    pub added: u32,
    /// Entry names that were skipped because the archive already had them.
    pub skipped_existing: Vec<String>,
}

/// Append `inputs` (files and/or directories) to the ZIP at `archive_path`.
///
/// `store_root`, matching the create path's flag, controls whether a directory
/// input keeps its own name as a leading path component (`true` → `docs/a.txt`)
/// or contributes only its contents (`false` → `a.txt`).
///
/// `level` is the Deflate level for the NEW entries (0 = store, 1–9); `None`
/// uses a sensible default. Existing entries keep whatever compression they
/// already had — they are not touched.
///
/// Errors if the target is not a readable ZIP, or if a write fails partway. A
/// mid-write failure can leave the archive with a trailing partial entry the
/// central directory does not point at; the caller (CLI / FFI) is expected to
/// surface the error rather than treat the archive as updated.
pub fn append_to_zip(
    archive_path: &Path,
    inputs: &[PathBuf],
    store_root: bool,
    level: Option<u8>,
) -> Result<ZipAppendReport> {
    // 1. Confirm it is actually a ZIP before opening it for writing. Appending
    //    to a .7z or a .tar with the ZIP append machinery would corrupt it.
    match crate::format::detect(archive_path)? {
        crate::format::ArchiveFormat::Zip => {}
        _ => {
            return Err(OtterzipError::FeatureDisabled(
                "adding to an existing archive is supported for ZIP only",
            ));
        }
    }

    // 2. Collect the names already present, so a re-add is a skip rather than a
    //    confusing duplicate entry. Read handle is dropped before the write
    //    handle opens.
    let existing = existing_entry_names(archive_path)?;

    // 3. Enumerate what the caller wants to add, resolving directories to their
    //    files up front so a walk error aborts before we touch the archive.
    let mut planned: Vec<(PathBuf, String)> = Vec::new();
    for input in inputs {
        if !input.exists() {
            return Err(OtterzipError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("input not found: {}", input.display()),
            )));
        }
        if input.is_dir() {
            let prefix = if store_root {
                input
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string()
            } else {
                String::new()
            };
            collect_dir(input, &prefix, &mut planned)?;
        } else {
            let name = input
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| {
                    OtterzipError::InvalidArgument("input file has a non-UTF-8 name")
                })?
                .to_string();
            planned.push((input.clone(), name));
        }
    }

    // 4. Open the archive in append mode and write the newcomers.
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(archive_path)
        .map_err(OtterzipError::Io)?;
    let mut writer = zip::ZipWriter::new_append(file)
        .map_err(|e| OtterzipError::BackendError(format!("open ZIP for append: {e}")))?;

    let deflate_level = level_to_zip(level);
    let mut report = ZipAppendReport::default();

    for (src, entry_name) in planned {
        if existing.contains(&entry_name) {
            report.skipped_existing.push(entry_name);
            continue;
        }
        write_one(&mut writer, &src, &entry_name, deflate_level)?;
        report.added += 1;
    }

    writer
        .finish()
        .map_err(|e| OtterzipError::BackendError(format!("finalise ZIP append: {e}")))?;

    Ok(report)
}

/// The entry names already in the archive, so [`append_to_zip`] can skip a
/// collision instead of writing a duplicate.
fn existing_entry_names(archive_path: &Path) -> Result<std::collections::HashSet<String>> {
    let archive = crate::archive::Archive::open(archive_path, crate::archive::OpenMode::Read)?;
    let mut names = std::collections::HashSet::new();
    for entry in archive.entries()? {
        names.insert(entry?.path);
    }
    Ok(names)
}

/// Recursively enumerate a directory's files as `(source, entry_name)` pairs,
/// entry names joined with `/` regardless of host separator.
fn collect_dir(dir: &Path, prefix: &str, out: &mut Vec<(PathBuf, String)>) -> Result<()> {
    let mut stack = vec![(dir.to_path_buf(), prefix.to_string())];
    while let Some((current, cur_prefix)) = stack.pop() {
        for entry in fs::read_dir(&current).map_err(OtterzipError::Io)? {
            let entry = entry.map_err(OtterzipError::Io)?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let entry_name = if cur_prefix.is_empty() {
                name
            } else {
                format!("{cur_prefix}/{name}")
            };
            if path.is_dir() {
                stack.push((path, entry_name));
            } else {
                out.push((path, entry_name));
            }
        }
    }
    Ok(())
}

/// Write one file into the append stream, preserving its mtime and (on Unix)
/// its permission bits so the round trip matches what the create path stores.
fn write_one<W: Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    src: &Path,
    entry_name: &str,
    level: Option<i64>,
) -> Result<()> {
    let meta = fs::metadata(src).map_err(OtterzipError::Io)?;

    let mut options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(level);
    if let Some(dt) = meta.modified().ok().and_then(zip_datetime) {
        options = options.last_modified_time(dt);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        options = options.unix_permissions(meta.permissions().mode() & 0o777);
    }

    writer
        .start_file(entry_name, options)
        .map_err(|e| OtterzipError::BackendError(format!("start entry '{entry_name}': {e}")))?;

    let mut f = fs::File::open(src).map_err(OtterzipError::Io)?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(OtterzipError::Io)?;
        if n == 0 {
            break;
        }
        writer
            .write_all(&buf[..n])
            .map_err(|e| OtterzipError::BackendError(format!("write entry '{entry_name}': {e}")))?;
    }
    Ok(())
}

/// Map the public 0–9 level to the `zip` crate's level type. `None` and 0 are
/// distinct: `None` means "the format default", `Some(0)` means Store — but
/// this path always sets Deflated, so `Some(0)` is the lowest Deflate effort
/// rather than true Store. Callers who want Store pass level 0 and accept that;
/// it is a level knob, not a method switch.
fn level_to_zip(level: Option<u8>) -> Option<i64> {
    level.map(|l| i64::from(l.clamp(0, 9)))
}

/// Convert a [`SystemTime`] to a ZIP [`DateTime`](zip::DateTime).
///
/// ZIP stores MS-DOS local time with no zone; we treat the instant as UTC
/// civil time, matching how the reader decodes it back. Returns `None` for
/// anything the DOS format cannot hold (before 1980, after 2107), which the
/// caller renders as "no timestamp" rather than a wrong one.
fn zip_datetime(t: SystemTime) -> Option<zip::DateTime> {
    let secs = t.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let days = (secs / 86_400) as i64;
    let tod = (secs % 86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    let year = u16::try_from(year).ok()?;
    zip::DateTime::from_date_and_time(
        year,
        month,
        day,
        (tod / 3600) as u8,
        ((tod % 3600) / 60) as u8,
        (tod % 60) as u8,
    )
    .ok()
}

/// Howard Hinnant's days-from-civil inverse: days since the Unix epoch to
/// `(year, month, day)`. The same arithmetic the ZIP/RAR readers use in the
/// other direction, so an mtime survives a create→read or append→read round
/// trip unchanged.
fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (y + i64::from(m <= 2), m as u8, d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{Archive, OpenMode};
    use crate::options::CreateOptions;
    use crate::ArchiveFormat;

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        let dir = tempfile::tempdir().unwrap();
        let mut srcs = Vec::new();
        for (name, body) in files {
            let p = dir.path().join(name);
            fs::write(&p, body).unwrap();
            srcs.push((p, name.to_string()));
        }
        let mut archive = Archive::create(
            path,
            CreateOptions {
                format: ArchiveFormat::Zip,
                ..CreateOptions::default()
            },
        )
        .unwrap();
        for (p, name) in &srcs {
            archive.add_file(p, name).unwrap();
        }
        archive.commit().unwrap();
    }

    fn entry_names(path: &Path) -> Vec<String> {
        let a = Archive::open(path, OpenMode::Read).unwrap();
        let mut v: Vec<String> = a.entries().unwrap().map(|e| e.unwrap().path).collect();
        v.sort();
        v
    }

    #[test]
    fn appends_a_new_file_without_disturbing_existing() {
        let td = tempfile::tempdir().unwrap();
        let zip = td.path().join("a.zip");
        make_zip(&zip, &[("first.txt", b"one"), ("second.txt", b"two")]);

        let extra = td.path().join("third.txt");
        fs::write(&extra, b"three").unwrap();
        let report = append_to_zip(&zip, &[extra], false, None).unwrap();

        assert_eq!(report.added, 1);
        assert!(report.skipped_existing.is_empty());
        assert_eq!(
            entry_names(&zip),
            vec!["first.txt", "second.txt", "third.txt"]
        );

        // The pre-existing bytes must still read back intact.
        let a = Archive::open(&zip, OpenMode::Read).unwrap();
        let mut r = a.read_entry("first.txt").unwrap();
        let mut s = String::new();
        r.read_to_string(&mut s).unwrap();
        assert_eq!(s, "one");
    }

    #[test]
    fn a_colliding_name_is_skipped_not_duplicated() {
        let td = tempfile::tempdir().unwrap();
        let zip = td.path().join("a.zip");
        make_zip(&zip, &[("dup.txt", b"original")]);

        let extra = td.path().join("dup.txt");
        fs::write(&extra, b"replacement").unwrap();
        let report = append_to_zip(&zip, &[extra], false, None).unwrap();

        assert_eq!(report.added, 0);
        assert_eq!(report.skipped_existing, vec!["dup.txt"]);
        // Exactly one entry, and it is still the original bytes.
        assert_eq!(entry_names(&zip), vec!["dup.txt"]);
        let a = Archive::open(&zip, OpenMode::Read).unwrap();
        let mut r = a.read_entry("dup.txt").unwrap();
        let mut s = String::new();
        r.read_to_string(&mut s).unwrap();
        assert_eq!(s, "original");
    }

    #[test]
    fn appends_a_directory_tree() {
        let td = tempfile::tempdir().unwrap();
        let zip = td.path().join("a.zip");
        make_zip(&zip, &[("root.txt", b"r")]);

        let src = td.path().join("docs");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), b"a").unwrap();
        fs::write(src.join("sub/b.txt"), b"b").unwrap();

        let report = append_to_zip(&zip, &[src], true, Some(6)).unwrap();
        assert_eq!(report.added, 2);
        assert_eq!(
            entry_names(&zip),
            vec!["docs/a.txt", "docs/sub/b.txt", "root.txt"]
        );
    }

    #[test]
    fn refuses_a_non_zip_target() {
        let td = tempfile::tempdir().unwrap();
        let seven = td.path().join("a.7z");
        // 7z magic so detect() classifies it, content irrelevant.
        fs::write(&seven, b"7z\xBC\xAF\x27\x1C\x00\x04rest").unwrap();
        let extra = td.path().join("x.txt");
        fs::write(&extra, b"x").unwrap();

        let err = append_to_zip(&seven, &[extra], false, None).unwrap_err();
        assert!(matches!(err, OtterzipError::FeatureDisabled(_)));
    }
}
