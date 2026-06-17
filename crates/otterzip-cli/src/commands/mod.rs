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
use std::path::Path;

/// Open an archive read-only, with a password when one is supplied.
/// Shared by extract / test / list.
pub(crate) fn open_archive(path: &Path, password: Option<&str>) -> Result<Archive> {
    let archive = match password {
        Some(pw) => Archive::open_with_password(path, OpenMode::Read, pw.to_string())?,
        None => Archive::open(path, OpenMode::Read)?,
    };
    Ok(archive)
}
