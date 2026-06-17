//! `OtterzipError` → process exit-code mapping.
//!
//! Stable, documented codes so scripts can branch on failure kind.
//! Mirrors the table in `docs/03-api/cli.md`. CLI-level usage errors
//! (bad flags, existing-target guard) surface as plain `anyhow` errors
//! and map to [`EXIT_USAGE`] in `main`.

use otterzip_core::OtterzipError;

pub const EXIT_OK: i32 = 0;
pub const EXIT_USAGE: i32 = 2;

/// Map a core error to its documented exit code.
#[must_use]
pub fn code_for(err: &OtterzipError) -> i32 {
    match err {
        OtterzipError::InvalidArgument(_) => 2,
        OtterzipError::Io(_) => 3,
        OtterzipError::WrongPassword => 4,
        OtterzipError::Corrupted { .. } => 5,
        OtterzipError::EntryNotFound(_) => 6,
        OtterzipError::UnsupportedFormat(_) => 7,
        OtterzipError::FeatureDisabled(_) => 8,
        OtterzipError::MissingVolume { .. } => 9,
        OtterzipError::PathTraversalBlocked(_) => 10,
        OtterzipError::ZipBombSuspected { .. } => 11,
        OtterzipError::BackendError(_) => 12,
        OtterzipError::Canceled => 130,
    }
}

/// Best-effort exit code for an `anyhow` error: if it wraps a core
/// `OtterzipError` use the typed mapping, else treat as a usage error.
#[must_use]
pub fn code_for_anyhow(err: &anyhow::Error) -> i32 {
    err.downcast_ref::<OtterzipError>()
        .map_or(EXIT_USAGE, code_for)
}
