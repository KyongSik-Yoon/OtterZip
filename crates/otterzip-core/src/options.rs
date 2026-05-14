//! Extract/Create option types. See `rust-core-api.md` §1.5.

use std::path::PathBuf;

use zeroize::Zeroizing;

use crate::format::{ArchiveFormat, CompressionMethod, EncryptionMethod};

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OverwritePolicy {
    Never = 0,
    Always = 1,
    IfNewer = 2,
    AskCallback = 3,
}

#[derive(Debug, Clone)]
pub struct ExtractOptions {
    pub destination: PathBuf,
    pub overwrite: OverwritePolicy,
    pub flatten_paths: bool,
    pub preserve_permissions: bool,
    pub preserve_timestamps: bool,
    pub follow_symlinks: bool,
    pub block_path_traversal: bool,
    /// ZIP-bomb defence. Per `domain-model.md` §5: when an entry's
    /// uncompressed-to-compressed ratio strictly exceeds this threshold,
    /// extraction aborts with [`crate::OtterzipError::BackendError`] tagged
    /// `"zip-bomb-suspected"`. `0` disables the check entirely. Default 1000
    /// matches the documented heuristic; legitimate text-heavy archives
    /// rarely exceed ~200:1 so 1000 is a safe headroom.
    pub max_compression_ratio: u32,
    /// Phase 7 — second-line zip-bomb defence. When the cumulative
    /// uncompressed/compressed ratio across all entries exceeds this
    /// threshold, extraction aborts. Per-entry attacks fly under the
    /// `max_compression_ratio` radar by spreading data across many small
    /// entries; this gate watches the aggregate. `0` disables it.
    pub max_total_compression_ratio: u32,
    /// Phase 7 — absolute cap on bytes written during a single
    /// `extract_all` call. Prevents pathological archives from filling
    /// the disk regardless of ratio. `0` disables.
    pub max_total_output_bytes: u64,
    pub entry_filter: Option<Vec<String>>,
    pub password: Option<Zeroizing<String>>,
    /// Phase 7 (PR-7A) — propagate Mark-of-the-Web (Zone.Identifier ADS)
    /// from the source archive to every extracted file. When the archive
    /// itself carries `archive.zip:Zone.Identifier` (e.g. from a browser
    /// download), we duplicate that ADS onto each output file so Windows'
    /// SmartScreen / UAC keep showing the right warnings on subsequent
    /// execution.
    ///
    /// Default: `true` (security-first). Toggle: `Settings_PreserveZoneIdentifier`.
    /// Windows-only; on other platforms the writer is a no-op regardless.
    pub preserve_zone_identifier: bool,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            destination: PathBuf::new(),
            overwrite: OverwritePolicy::Never,
            flatten_paths: false,
            preserve_permissions: true,
            preserve_timestamps: true,
            follow_symlinks: false,
            block_path_traversal: true,
            max_compression_ratio: 1000,
            // Cumulative ratio threshold. 100:1 is generous for legitimate
            // text-heavy archives (Silesia averages ~3-4:1) but stops the
            // typical "1 GB unpacks to 4 PB" zip bombs cold.
            max_total_compression_ratio: 100,
            // Hard ceiling: 16 GiB. Larger legitimate archives are rare;
            // callers who need more pass the explicit override.
            max_total_output_bytes: 16 * 1024 * 1024 * 1024,
            entry_filter: None,
            password: None,
            preserve_zone_identifier: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub format: ArchiveFormat,
    pub compression: CompressionMethod,
    pub compression_level: u8,
    pub encryption: EncryptionMethod,
    pub password: Option<Zeroizing<String>>,
    pub volume_size_bytes: Option<u64>,
    pub solid: bool,
    pub preserve_permissions: bool,
    pub preserve_timestamps: bool,
    pub thread_count: Option<u32>,
    /// Phase 6+ — skip filesystem-noise files when adding a directory tree.
    /// Matches the basenames listed in [`SYSTEM_METADATA_FILES`] and the
    /// folder names in [`SYSTEM_METADATA_FOLDERS`] (case-insensitive on
    /// Windows-style platforms; we apply ASCII case-insensitive everywhere
    /// for predictable cross-platform behavior). AppleDouble shadow files
    /// (`._<name>`) are also dropped via prefix match.
    ///
    /// Default: `true`. The toggle lives in `Settings_ExcludeSystemMetadata`.
    pub exclude_system_metadata: bool,
}

impl Default for CreateOptions {
    fn default() -> Self {
        Self {
            format: ArchiveFormat::Zip,
            compression: CompressionMethod::Deflate,
            // Level 3 is the sweet-spot Bandizip uses as its
            // "Fast Drag and Drop" baseline (their public 3 min 22 s
            // wall-clock on the user's 9.5 GB reproducer was measured
            // against that preset). Throughput is ~2× level 5 / ~3×
            // level 9 on real deflate-friendly inputs, while archive
            // size grows only 3–5 % on average corpora (much of the
            // user's input is already-compressed payload that
            // smart-store routes around anyway, so the size delta
            // ends up smaller than the synthetic-fixture worst case).
            compression_level: 3,
            encryption: EncryptionMethod::None,
            password: None,
            volume_size_bytes: None,
            solid: false,
            preserve_permissions: true,
            preserve_timestamps: true,
            thread_count: None,
            exclude_system_metadata: true,
        }
    }
}

/// Basenames that count as OS noise. Matched case-insensitively (ASCII).
pub const SYSTEM_METADATA_FILES: &[&str] = &[
    ".DS_Store",
    "Thumbs.db",
    "ehthumbs.db",
    "ehthumbs_vista.db",
    "desktop.ini",
];

/// Folder names that count as OS noise. Matched case-insensitively (ASCII)
/// against any path component, so `parent/$Recycle.Bin/foo.txt` is excluded.
pub const SYSTEM_METADATA_FOLDERS: &[&str] = &[
    "$RECYCLE.BIN",
    "$Recycle.Bin",
    "System Volume Information",
    ".Spotlight-V100",
    ".Trashes",
    ".AppleDouble",
    ".fseventsd",
    ".TemporaryItems",
];

/// Returns `true` if the given path's basename / any component matches the
/// OS-noise lists above, or if the basename is an AppleDouble shadow
/// (`._<rest>`).
pub fn is_system_metadata(path: &std::path::Path) -> bool {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.starts_with("._") {
            return true;
        }
        if SYSTEM_METADATA_FILES
            .iter()
            .any(|f| name.eq_ignore_ascii_case(f))
        {
            return true;
        }
    }
    for comp in path.components() {
        if let std::path::Component::Normal(s) = comp {
            if let Some(s) = s.to_str() {
                if SYSTEM_METADATA_FOLDERS
                    .iter()
                    .any(|f| s.eq_ignore_ascii_case(f))
                {
                    return true;
                }
            }
        }
    }
    false
}
