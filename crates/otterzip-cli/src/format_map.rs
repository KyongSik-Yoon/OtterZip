//! Format-string ↔ `ArchiveFormat` mapping and level → codec selection.
//!
//! `otterzip-core` deliberately exposes no `FromStr` for [`ArchiveFormat`]
//! (detection is magic-byte based, see `format::detect`). The CLI needs
//! the inverse — turn a user-typed `--format zip` / an output filename
//! into the enum — so we keep a single source-of-truth table here.

use anyhow::{bail, Result};
use otterzip_core::{ArchiveFormat, CompressionMethod};

/// Recognised create-capable format tokens → enum. Compound tokens
/// (`tar.gz`) and their short aliases (`tgz`) both map to the same
/// variant. Keep this list aligned with `format::detect`'s extension
/// disambiguation and the manifest FileTypeAssociation block.
const FORMAT_TABLE: &[(&str, ArchiveFormat)] = &[
    ("zip", ArchiveFormat::Zip),
    ("zipx", ArchiveFormat::Zipx),
    ("7z", ArchiveFormat::SevenZ),
    ("sevenz", ArchiveFormat::SevenZ),
    ("tar", ArchiveFormat::Tar),
    ("gz", ArchiveFormat::Gzip),
    ("gzip", ArchiveFormat::Gzip),
    ("tgz", ArchiveFormat::TarGz),
    ("tar.gz", ArchiveFormat::TarGz),
    ("bz2", ArchiveFormat::Bzip2),
    ("bzip2", ArchiveFormat::Bzip2),
    ("tbz", ArchiveFormat::TarBz2),
    ("tbz2", ArchiveFormat::TarBz2),
    ("tar.bz2", ArchiveFormat::TarBz2),
    ("xz", ArchiveFormat::Xz),
    ("txz", ArchiveFormat::TarXz),
    ("tar.xz", ArchiveFormat::TarXz),
    ("lzma", ArchiveFormat::Lzma),
    ("zst", ArchiveFormat::Zstd),
    ("zstd", ArchiveFormat::Zstd),
    ("tzst", ArchiveFormat::TarZst),
    ("tar.zst", ArchiveFormat::TarZst),
    ("lz4", ArchiveFormat::Lz4),
    ("tar.lz4", ArchiveFormat::TarLz4),
];

/// Parse an explicit `--format` token. Leading dot and case are
/// tolerated (`.ZIP` → Zip). Read-only / license-restricted formats
/// produce a specific, actionable error rather than "unknown".
pub fn parse_format(s: &str) -> Result<ArchiveFormat> {
    let key = s.trim().trim_start_matches('.').to_ascii_lowercase();
    match key.as_str() {
        "rar" => {
            bail!("RAR creation is not supported (license-restricted; OtterZip extracts RAR only)")
        }
        "iso" | "cab" | "msi" | "deb" => {
            bail!("'{key}' is an extract-only format; cannot create archives in it")
        }
        _ => {}
    }
    FORMAT_TABLE
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, f)| *f)
        .ok_or_else(|| {
            anyhow::anyhow!("unknown archive format '{s}' (try: zip, 7z, tar.gz, tar, zstd)")
        })
}

/// Infer the format from an output archive's filename when `--format`
/// is omitted. Compound extensions (`.tar.gz`) win over their single
/// tail (`.gz`) because we test the longest tokens first. Falls back to
/// ZIP for unrecognised names.
pub fn format_from_output_ext(path: &std::path::Path) -> ArchiveFormat {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // Longest token first so `.tar.gz` matches before `.gz`.
    let mut tokens: Vec<&(&str, ArchiveFormat)> = FORMAT_TABLE.iter().collect();
    tokens.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    for (k, f) in tokens {
        if name.ends_with(&format!(".{k}")) {
            return *f;
        }
    }
    ArchiveFormat::Zip
}

/// Pick the compression method for a format at a given level. Level 0
/// means "stored" (no compression) where the container supports it.
/// Levels 1–9 select the format-natural codec; the numeric level itself
/// rides separately in `CreateOptions::compression_level` (the core
/// clamps per-backend).
pub fn codec_for(format: ArchiveFormat, level: u8) -> CompressionMethod {
    use ArchiveFormat as F;
    use CompressionMethod as C;
    if level == 0 {
        return C::Store;
    }
    match format {
        F::Zip | F::Gzip | F::TarGz => C::Deflate,
        F::Zipx => C::Bzip2,
        F::SevenZ | F::Xz | F::TarXz => C::Lzma2,
        F::Bzip2 | F::TarBz2 => C::Bzip2,
        F::Lzma => C::Lzma,
        F::Zstd | F::TarZst => C::Zstd,
        // Tar is a pure container; Lz4 frames carry their own framing.
        // Either way the backend ignores the method, so Store is the
        // honest value.
        F::Tar | F::Lz4 | F::TarLz4 => C::Store,
        _ => C::Deflate,
    }
}

/// Canonical file extension for a format (no leading dot). Used by
/// `bc` (batch-create) to name `<input>.<ext>` per item.
#[must_use]
pub fn primary_ext(format: ArchiveFormat) -> &'static str {
    use ArchiveFormat as F;
    match format {
        F::Zip => "zip",
        F::Zipx => "zipx",
        F::SevenZ => "7z",
        F::Tar => "tar",
        F::Gzip => "gz",
        F::TarGz => "tar.gz",
        F::Bzip2 => "bz2",
        F::TarBz2 => "tar.bz2",
        F::Xz => "xz",
        F::TarXz => "tar.xz",
        F::Lzma => "lzma",
        F::Zstd => "zst",
        F::TarZst => "tar.zst",
        F::Lz4 => "lz4",
        F::TarLz4 => "tar.lz4",
        _ => "zip",
    }
}

/// Parse a volume size like `100m`, `1g`, `512k`, or a raw byte count.
pub fn parse_size(s: &str) -> Result<u64> {
    let lower = s.trim().to_ascii_lowercase();
    let (digits, mult) = match lower.as_bytes().last() {
        Some(b'g') => (&lower[..lower.len() - 1], 1u64 << 30),
        Some(b'm') => (&lower[..lower.len() - 1], 1u64 << 20),
        Some(b'k') => (&lower[..lower.len() - 1], 1u64 << 10),
        _ => (lower.as_str(), 1u64),
    };
    let n: u64 = digits
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid size '{s}' (use e.g. 100m, 1g, 512k)"))?;
    n.checked_mul(mult)
        .ok_or_else(|| anyhow::anyhow!("size '{s}' overflows u64"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_tokens() {
        assert_eq!(parse_format("zip").unwrap(), ArchiveFormat::Zip);
        assert_eq!(parse_format(".ZIP").unwrap(), ArchiveFormat::Zip);
        assert_eq!(parse_format("7z").unwrap(), ArchiveFormat::SevenZ);
        assert_eq!(parse_format("tar.gz").unwrap(), ArchiveFormat::TarGz);
        assert_eq!(parse_format("tgz").unwrap(), ArchiveFormat::TarGz);
        assert_eq!(parse_format("zstd").unwrap(), ArchiveFormat::Zstd);
    }

    #[test]
    fn rejects_readonly_and_unknown() {
        assert!(parse_format("rar").is_err());
        assert!(parse_format("iso").is_err());
        assert!(parse_format("wat").is_err());
    }

    #[test]
    fn infers_from_filename_compound_first() {
        use std::path::Path;
        assert_eq!(
            format_from_output_ext(Path::new("a.tar.gz")),
            ArchiveFormat::TarGz
        );
        assert_eq!(
            format_from_output_ext(Path::new("a.gz")),
            ArchiveFormat::Gzip
        );
        assert_eq!(
            format_from_output_ext(Path::new("a.7z")),
            ArchiveFormat::SevenZ
        );
        assert_eq!(
            format_from_output_ext(Path::new("noext")),
            ArchiveFormat::Zip
        );
    }

    #[test]
    fn level_zero_is_store() {
        assert_eq!(codec_for(ArchiveFormat::Zip, 0), CompressionMethod::Store);
        assert_eq!(codec_for(ArchiveFormat::Zip, 5), CompressionMethod::Deflate);
        assert_eq!(
            codec_for(ArchiveFormat::SevenZ, 5),
            CompressionMethod::Lzma2
        );
    }

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("100m").unwrap(), 100 * (1 << 20));
        assert_eq!(parse_size("1g").unwrap(), 1 << 30);
        assert_eq!(parse_size("512").unwrap(), 512);
        assert!(parse_size("abc").is_err());
    }
}
