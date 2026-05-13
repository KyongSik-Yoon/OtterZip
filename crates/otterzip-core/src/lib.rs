//! OtterZip core — archive format library.
//!
//! Public API surface is documented in `docs/03-api/rust-core-api.md`.
//! This crate is the primary consumer target for `otterzip-ffi`.

#![warn(
    clippy::pedantic,
    clippy::nursery,
    clippy::inefficient_to_string,
    clippy::large_stack_arrays,
    clippy::large_types_passed_by_value,
    clippy::mut_mut,
    clippy::needless_pass_by_value,
    clippy::redundant_allocation,
    clippy::unnecessary_box_returns,
    clippy::useless_conversion,
    clippy::undocumented_unsafe_blocks,
)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
)]

pub mod archive;
pub mod encoding;
pub mod entry;
pub mod error;
pub mod format;
pub mod motw;
pub mod options;
pub mod progress;

mod backends;

// Re-exports — crate root shortcuts
pub use archive::{Archive, ExtractReport, ExtractWarning, OpenMode, TestReport, VolumeInfo};
pub use entry::{Entry, EntryIter, HostOs};
pub use error::{Result, OtterzipError};
pub use format::{detect, detect_bytes, ArchiveFormat, CompressionMethod, EncryptionMethod};
pub use options::{CreateOptions, ExtractOptions, OverwritePolicy};
pub use progress::{Progress, ProgressPhase, ProgressSink};

/// Crate version string.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Diagnostic-only re-export of the ZIP sanity-check helper so the
/// `probe_archive` example can verify the pre-flight verdict on real
/// user archives without going through the full Archive::open dance.
/// Not part of the stable surface — gated under `__probe` doc-hidden.
#[doc(hidden)]
pub fn __probe_quick_eocd_sanity(path: &std::path::Path) -> std::result::Result<(), String> {
    backends::zip::__probe_quick_eocd_sanity_check(path)
}

/// Test-only entry point for the Day-1 lenient ZIP parser. Returns
/// the `(path, uncompressed_size)` pairs the central-directory walk
/// produced. Used by `tests/lenient_zip.rs` to assert parity with the
/// strict `ZipBackend` — the dispatcher's fallback arm only fires on
/// malformed archives, but parity must hold for healthy ones too or
/// the v1.0 swap will silently lose entries the user expected.
/// Not part of the stable surface.
#[doc(hidden)]
pub fn __probe_lenient_entries(
    path: &std::path::Path,
) -> Result<Vec<(String, u64)>> {
    backends::lenient_zip::__probe_entries(path)
}
