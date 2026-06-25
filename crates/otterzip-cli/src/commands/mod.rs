//! Command handlers. Each `run` returns the process exit code on the
//! success path (e.g. `t` returns 5 when corrupted) or an `anyhow`
//! error that `main` maps via `exit::code_for_anyhow`.

pub mod add;
pub mod batch;
pub mod extract;
pub mod list;
pub mod test;

use anyhow::Result;
use otterzip_core::{Archive, OpenMode};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

/// Open an archive read-only, with a password when one is supplied.
/// Shared by extract / test / list.
///
/// Raw byte-split inputs (`name.7z.001`, `name.zip.001`, …) — the form
/// `otterzip a -v` produces — are transparently merged: the sibling
/// volumes are concatenated into a temp file that is opened in their
/// place (the core's `open_multi` only reads spanned ZIP, so this
/// concat-then-open mirrors what the GUI does and works for every
/// format, including split 7z). The returned [`NamedTempFile`], when
/// present, MUST be held by the caller for the lifetime of the
/// [`Archive`] — dropping it deletes the merged copy.
pub(crate) fn open_archive(
    path: &Path,
    password: Option<&str>,
) -> Result<(Archive, Option<NamedTempFile>)> {
    let (open_path, merged) = resolve_split_input(path)?;
    let archive = match password {
        Some(pw) => Archive::open_with_password(&open_path, OpenMode::Read, pw.to_string())?,
        None => Archive::open(&open_path, OpenMode::Read)?,
    };
    Ok((archive, merged))
}

/// If `path` is the first volume of a raw byte-split set (`*.001` with at
/// least one sibling `*.002`), concatenate every contiguous volume into a
/// temp file and return its path. Otherwise return `path` unchanged.
fn resolve_split_input(path: &Path) -> Result<(PathBuf, Option<NamedTempFile>)> {
    if path.extension().and_then(|e| e.to_str()) != Some("001") {
        return Ok((path.to_path_buf(), None));
    }
    let segments = discover_raw_split_segments(path);
    if segments.len() < 2 {
        return Ok((path.to_path_buf(), None)); // lone ".001" — open as-is
    }
    // Give the temp the real archive extension (.7z / .zip) so any
    // extension-based handling still works; detection is by magic bytes.
    let suffix = path
        .with_extension("")
        .extension()
        .and_then(|e| e.to_str())
        .map_or_else(String::new, |e| format!(".{e}"));
    let mut tmp = tempfile::Builder::new()
        .prefix("otterzip-merge-")
        .suffix(&suffix)
        .tempfile()?;
    for seg in &segments {
        let mut f = std::fs::File::open(seg)?;
        std::io::copy(&mut f, tmp.as_file_mut())?;
    }
    tmp.as_file_mut().flush()?;
    Ok((tmp.path().to_path_buf(), Some(tmp)))
}

/// Collect `base.001`, `base.002`, … (contiguous) where `path` is the
/// `.001` volume; stops at the first missing index.
fn discover_raw_split_segments(path: &Path) -> Vec<PathBuf> {
    let base = path.with_extension(""); // strip the ".001" extension
    let mut segments = Vec::new();
    for i in 1..=999u32 {
        let mut name: OsString = base.clone().into_os_string();
        name.push(format!(".{i:03}"));
        let seg = PathBuf::from(name);
        if seg.is_file() {
            segments.push(seg);
        } else {
            break;
        }
    }
    segments
}
