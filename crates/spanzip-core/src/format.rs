//! Archive format identifiers. See `schema.md` §1.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::error::Result;

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ArchiveFormat {
    Unknown = 0,
    Zip = 1,
    SevenZ = 2,
    Rar = 3,
    Tar = 4,
    Gzip = 5,
    TarGz = 6,
    TarBz2 = 7,
    TarXz = 8,

    // ABI v7 (Phase 7+ — option Y, see CHANGELOG-ffi.md / schema.md §1.1).
    // Slot 12 is reserved (Compress/.Z was OUT'd at the PoC gate).
    Bzip2 = 9,
    Xz = 10,
    Lzma = 11,
    // PR-F2 — next-gen single-stream + tar wrappers.
    Zstd = 13,   // .zst single stream
    TarZst = 14, // .tar.zst
    Lz4 = 15,    // .lz4 (frame format)
    TarLz4 = 16, // .tar.lz4
    // PR-F5 — ISO9660 disk image (read-only). Joliet + Rock Ridge
    // extensions handled by the iso9660-rs backend; UDF is detected
    // separately and returns Unsupported (deferred to v1.1).
    Iso = 18,    // slot 17 (Zipx) is reserved for a later PR
    // PR-F6 — Windows installer family (read-only). CAB is a
    // standalone container; MSI is an OLE2 Compound Document that
    // typically embeds one or more CAB streams plus tabular metadata.
    Cab = 19,
    Msi = 20,
    // PR-F8 — Debian package = ar(1) container holding `debian-binary`
    // + `control.tar.*` + `data.tar.*`. We expose the three ar members
    // directly; users who want the inner files can re-open the
    // extracted .tar.* with SpanZIP.
    Deb = 21,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CompressionMethod {
    Store = 0,
    Deflate = 1,
    Deflate64 = 2,
    Bzip2 = 3,
    Lzma = 4,
    Lzma2 = 5,
    Zstd = 6,
    Ppmd = 7,
    Unknown = 0xFFFF_FFFF,
}

#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EncryptionMethod {
    None = 0,
    ZipCrypto = 1,
    Aes128 = 2,
    Aes256 = 3,
}

/// Size of the prefix buffer we sniff for magic bytes. 512 bytes covers
/// everything we need: TAR ustar magic sits at offset 257, plus we leave
/// headroom for future format signatures. Stack-allocated — no heap alloc.
const SNIFF_LEN: usize = 512;

/// Magic byte constants (no hot-loop allocation — these are `&'static [u8]`).
const MAGIC_ZIP: &[u8] = &[0x50, 0x4B, 0x03, 0x04];
const MAGIC_ZIP_EMPTY: &[u8] = &[0x50, 0x4B, 0x05, 0x06]; // empty archive EOCD
const MAGIC_ZIP_SPAN: &[u8] = &[0x50, 0x4B, 0x07, 0x08]; // spanned marker
const MAGIC_7Z: &[u8] = &[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];
const MAGIC_RAR5: &[u8] = &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00];
const MAGIC_RAR4: &[u8] = &[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
const MAGIC_GZIP: &[u8] = &[0x1F, 0x8B];
const MAGIC_BZIP2: &[u8] = b"BZh";
const MAGIC_XZ: &[u8] = &[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00];
// PR-F2 — Zstandard frame magic (RFC 8478 §3.1.1) and LZ4 frame magic
// (LZ4 frame format §3). Both are 4-byte little-endian sentinels that
// uniquely identify their respective stream containers.
const MAGIC_ZSTD: &[u8] = &[0x28, 0xB5, 0x2F, 0xFD];
const MAGIC_LZ4_FRAME: &[u8] = &[0x04, 0x22, 0x4D, 0x18];
// PR-F6 — Microsoft Cabinet header magic ("MSCF") and OLE2 / Compound
// File Binary signature (used by MSI, .doc, .xls, .ppt). The CFB
// signature is shared by every OLE2 product so an extension check is
// still required to distinguish MSI from a Word doc; that's why we
// route the signature to ArchiveFormat::Msi *only* when the extension
// agrees (handled in `detect`).
const MAGIC_CAB: &[u8] = b"MSCF";
const MAGIC_OLE2: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
// PR-F8 — Unix `ar` archive header. Shared between BSD ar / GNU ar /
// Debian packages. We tentatively classify as Deb when this magic
// hits; `upgrade_with_extension` keeps it only for `.deb` (the bare
// .a / .ar static-library cases stay Unknown -- those aren't an
// archive product users right-click to extract).
const MAGIC_AR: &[u8] = b"!<arch>\n";

const TAR_USTAR_OFFSET: usize = 257;
const TAR_USTAR_MAGIC: &[u8] = b"ustar";

/// Detect format from a path (magic bytes preferred, extension as hint).
///
/// Reads up to 512 bytes from the head of the file and applies the byte
/// classifier in [`detect_bytes`]. When the magic alone can't disambiguate
/// (e.g. TAR inside GZIP vs a lone gzipped file), the file extension breaks
/// the tie. Returns [`ArchiveFormat::Unknown`] only when no signature and
/// no extension hint match — callers treat that as unsupported.
pub fn detect(path: impl AsRef<Path>) -> Result<ArchiveFormat> {
    let path = path.as_ref();
    let mut buf = [0u8; SNIFF_LEN];
    let n = {
        let mut f = File::open(path)?;
        // Read::read may return fewer bytes than requested; a small file
        // (< 512 B) is fine — detect_bytes accepts short slices.
        read_up_to(&mut f, &mut buf)?
    };
    let prefix = &buf[..n];

    if let Some(fmt) = detect_bytes(prefix) {
        // Magic matched. GZIP / BZIP2 / XZ could still be a tarball — upgrade
        // based on extension hint since we don't want to decompress just to
        // peek at 512 bytes of the inner stream (performance.md §2 — no
        // gratuitous work).
        let upgraded = upgrade_with_extension(fmt, path);
        return Ok(upgraded);
    }

    // Last resort: trust the extension for ambiguous-but-unsigned formats
    // (plain TAR has no magic at offset 0, only at 257 — already handled
    // in detect_bytes, so this path is rare).
    if let Some(fmt) = extension_hint(path) {
        return Ok(fmt);
    }

    Ok(ArchiveFormat::Unknown)
}

/// Detect format from a byte prefix (streaming).
///
/// Returns `None` if the prefix does not match any known signature. The
/// caller is responsible for extension-based fallback / disambiguation.
#[must_use]
pub fn detect_bytes(prefix: &[u8]) -> Option<ArchiveFormat> {
    // ZIP: central-directory-less empty archives and spanned archives are
    // still ZIPs per APPNOTE.TXT §4.3.6.
    if starts_with(prefix, MAGIC_ZIP)
        || starts_with(prefix, MAGIC_ZIP_EMPTY)
        || starts_with(prefix, MAGIC_ZIP_SPAN)
    {
        return Some(ArchiveFormat::Zip);
    }
    if starts_with(prefix, MAGIC_7Z) {
        return Some(ArchiveFormat::SevenZ);
    }
    if starts_with(prefix, MAGIC_RAR5) || starts_with(prefix, MAGIC_RAR4) {
        return Some(ArchiveFormat::Rar);
    }
    if starts_with(prefix, MAGIC_XZ) {
        return Some(ArchiveFormat::TarXz); // tentative; narrowed by caller
    }
    if starts_with(prefix, MAGIC_GZIP) {
        return Some(ArchiveFormat::Gzip); // tentative; .tar.gz via extension
    }
    if starts_with(prefix, MAGIC_BZIP2) {
        return Some(ArchiveFormat::TarBz2); // tentative; plain .bz2 via ext
    }
    // PR-F2 — ZSTD / LZ4 frame magic. Tentatively classify as the single-
    // stream variant; `upgrade_with_extension` promotes to the TarZst /
    // TarLz4 case when the file extension says so.
    if starts_with(prefix, MAGIC_ZSTD) {
        return Some(ArchiveFormat::Zstd);
    }
    if starts_with(prefix, MAGIC_LZ4_FRAME) {
        return Some(ArchiveFormat::Lz4);
    }
    // PR-F6 — Microsoft Cabinet ("MSCF") is unambiguous.
    if starts_with(prefix, MAGIC_CAB) {
        return Some(ArchiveFormat::Cab);
    }
    // PR-F6 — OLE2 / CFB signature is shared with .doc / .xls / .ppt
    // and many other Microsoft formats, so we tentatively classify as
    // MSI. `upgrade_with_extension` keeps the MSI classification only
    // when the extension agrees; otherwise it falls back to Unknown
    // so the caller doesn't receive a misleading "MSI" for a Word
    // document.
    if starts_with(prefix, MAGIC_OLE2) {
        return Some(ArchiveFormat::Msi);
    }
    // PR-F8 — `ar` magic. Tentative Deb; the .deb extension check
    // in `upgrade_with_extension` keeps the classification only for
    // actual Debian packages (static libraries fall back to Unknown).
    if starts_with(prefix, MAGIC_AR) {
        return Some(ArchiveFormat::Deb);
    }

    // TAR: ustar magic at offset 257. Accept POSIX "ustar\0" and GNU "ustar ".
    if prefix.len() > TAR_USTAR_OFFSET + TAR_USTAR_MAGIC.len()
        && &prefix[TAR_USTAR_OFFSET..TAR_USTAR_OFFSET + TAR_USTAR_MAGIC.len()] == TAR_USTAR_MAGIC
    {
        return Some(ArchiveFormat::Tar);
    }

    None
}

/// Compare the leading bytes of `haystack` against `needle` without allocating.
fn starts_with(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && &haystack[..needle.len()] == needle
}

/// Read up to `buf.len()` bytes, returning the count actually read.
/// Short reads are not errors here — small archive files are legal.
fn read_up_to(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0usize;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// Narrow a tentative format with the file extension.
///
/// Magic bytes for `.gz`/`.bz2`/`.xz` are identical for plain single-stream
/// files and tarballs — we use the extension to split those cases.
fn upgrade_with_extension(tentative: ArchiveFormat, path: &Path) -> ArchiveFormat {
    let ext_lower = lowercase_extension(path);
    let ext = ext_lower.as_deref();
    let stem_lower = double_extension(path);
    let stem = stem_lower.as_deref();

    match tentative {
        ArchiveFormat::Gzip => {
            if matches!(ext, Some("tgz")) || matches!(stem, Some("tar.gz")) {
                ArchiveFormat::TarGz
            } else {
                ArchiveFormat::Gzip
            }
        }
        ArchiveFormat::TarBz2 => {
            // BZ2 magic ("BZh") is shared between plain and tar variants.
            // Phase 7+ option Y added an explicit Bzip2 variant for the
            // single-stream case; previously this branch fell back to
            // Unknown because the enum lacked a Bzip2-only slot.
            if matches!(ext, Some("tbz") | Some("tbz2")) || matches!(stem, Some("tar.bz2")) {
                ArchiveFormat::TarBz2
            } else if matches!(ext, Some("bz2")) {
                ArchiveFormat::Bzip2
            } else {
                ArchiveFormat::TarBz2
            }
        }
        ArchiveFormat::TarXz => {
            if matches!(ext, Some("txz")) || matches!(stem, Some("tar.xz")) {
                ArchiveFormat::TarXz
            } else if matches!(ext, Some("xz")) {
                ArchiveFormat::Xz
            } else {
                ArchiveFormat::TarXz
            }
        }
        // PR-F2 — promote to .tar.zst / .tar.lz4 when the extension says so.
        ArchiveFormat::Zstd => {
            if matches!(ext, Some("tzst")) || matches!(stem, Some("tar.zst")) {
                ArchiveFormat::TarZst
            } else {
                ArchiveFormat::Zstd
            }
        }
        ArchiveFormat::Lz4 => {
            if matches!(stem, Some("tar.lz4")) {
                ArchiveFormat::TarLz4
            } else {
                ArchiveFormat::Lz4
            }
        }
        // PR-F6 — OLE2 / CFB signature: keep `Msi` only when the
        // extension agrees. Word / Excel / PowerPoint share the
        // signature but aren't archive formats; misclassifying them
        // as MSI would cause confusing extraction failures.
        ArchiveFormat::Msi => {
            if matches!(ext, Some("msi")) {
                ArchiveFormat::Msi
            } else {
                ArchiveFormat::Unknown
            }
        }
        // PR-F8 — ar magic with non-.deb extension is most likely a
        // static library (.a / .ar) or BSD object archive, neither of
        // which we model as an "archive product". Fall back to
        // Unknown so the caller surfaces UnsupportedFormat.
        ArchiveFormat::Deb => {
            if matches!(ext, Some("deb")) {
                ArchiveFormat::Deb
            } else {
                ArchiveFormat::Unknown
            }
        }
        other => other,
    }
}

/// Fallback: derive format purely from the extension.
fn extension_hint(path: &Path) -> Option<ArchiveFormat> {
    let ext_lower = lowercase_extension(path);
    let stem_lower = double_extension(path);
    match (ext_lower.as_deref(), stem_lower.as_deref()) {
        (Some("zip"), _) => Some(ArchiveFormat::Zip),
        // PR-F3 — ZIP variant aliases. JAR / WAR / EAR (Java archives),
        // IPA (iOS payload), APK / AAB (Android), APPX / MSIX (Windows
        // app packages), XPI (Firefox extensions), CRX (Chrome v3+).
        // Every one of these is a ZIP container with extra structural
        // conventions on top, so the same ZIP backend handles them with
        // no per-format dispatch needed. UI label differentiation is a
        // host-side concern (see app/SpanZIP.App/UserControls/ConfigPanel).
        (Some("jar"), _)
        | (Some("war"), _)
        | (Some("ear"), _)
        | (Some("ipa"), _)
        | (Some("apk"), _)
        | (Some("aab"), _)
        | (Some("appx"), _)
        | (Some("msix"), _)
        | (Some("xpi"), _)
        | (Some("crx"), _) => Some(ArchiveFormat::Zip),
        (Some("7z"), _) => Some(ArchiveFormat::SevenZ),
        (Some("rar"), _) => Some(ArchiveFormat::Rar),
        (Some("tar"), _) => Some(ArchiveFormat::Tar),
        (Some("gz"), _) | (Some("tgz"), _) => {
            if matches!(stem_lower.as_deref(), Some("tar.gz"))
                || matches!(ext_lower.as_deref(), Some("tgz"))
            {
                Some(ArchiveFormat::TarGz)
            } else {
                Some(ArchiveFormat::Gzip)
            }
        }
        (Some("bz2"), _) | (Some("tbz"), _) | (Some("tbz2"), _) => {
            if matches!(stem_lower.as_deref(), Some("tar.bz2"))
                || matches!(ext_lower.as_deref(), Some("tbz") | Some("tbz2"))
            {
                Some(ArchiveFormat::TarBz2)
            } else {
                // Plain .bz2 single-stream (v7+).
                Some(ArchiveFormat::Bzip2)
            }
        }
        (Some("xz"), _) | (Some("txz"), _) => {
            if matches!(stem_lower.as_deref(), Some("tar.xz"))
                || matches!(ext_lower.as_deref(), Some("txz"))
            {
                Some(ArchiveFormat::TarXz)
            } else {
                // Plain .xz single-stream (v7+).
                Some(ArchiveFormat::Xz)
            }
        }
        // .lzma single-stream (v7+). No double-extension form is in scope —
        // .tar.lzma is rare and routes through the .tlz alias in TarBackend.
        (Some("lzma"), _) => Some(ArchiveFormat::Lzma),
        // PR-F4 — .tlz alias = tar + LZMA1 (legacy GNU naming).
        // We reuse the TarXz enum slot to avoid an ABI bump; the actual
        // LZMA1 vs LZMA2 dispatch happens in `backends::open_backend`
        // by inspecting the path extension. Safe because no other input
        // hits TarXz with a `.tlz` suffix.
        (Some("tlz"), _) => Some(ArchiveFormat::TarXz),
        // PR-F5 — ISO9660 disk images. Magic ('CD001' at offset 0x8001
        // = sector 16) is past the 512-byte sniff window, so detection
        // is extension-only here; the actual volume descriptor check
        // happens in `backends::iso::IsoBackend::open` which fails with
        // BackendError on a non-ISO9660 payload. `.img` is a
        // disk-image alias frequently used for raw / hybrid ISO9660
        // dumps. UDF (.udf) intentionally NOT routed here -- v1.1
        // backlog.
        (Some("iso"), _) | (Some("img"), _) => Some(ArchiveFormat::Iso),
        // PR-F6 — Windows installer family. Cabinet uses the "MSCF"
        // magic so detect_bytes already routes most cases; the
        // extension hint covers truncated / empty fixtures. MSI
        // shares the OLE2 signature with other Microsoft formats so
        // the extension is what disambiguates it.
        (Some("cab"), _) => Some(ArchiveFormat::Cab),
        (Some("msi"), _) => Some(ArchiveFormat::Msi),
        // PR-F8 — Debian package. The ar magic is checked at the
        // backend layer; here we only need the extension fallback
        // for truncated / empty .deb fixtures.
        (Some("deb"), _) => Some(ArchiveFormat::Deb),
        // PR-F2 — Zstd / LZ4 single-stream + tar variants.
        (Some("zst"), _) | (Some("tzst"), _) => {
            if matches!(stem_lower.as_deref(), Some("tar.zst"))
                || matches!(ext_lower.as_deref(), Some("tzst"))
            {
                Some(ArchiveFormat::TarZst)
            } else {
                Some(ArchiveFormat::Zstd)
            }
        }
        (Some("lz4"), _) => {
            if matches!(stem_lower.as_deref(), Some("tar.lz4")) {
                Some(ArchiveFormat::TarLz4)
            } else {
                Some(ArchiveFormat::Lz4)
            }
        }
        _ => None,
    }
}

/// Lowercase the single final extension, if any.
fn lowercase_extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
}

/// Return one of the recognised double extensions (`tar.gz`, `tar.bz2`,
/// `tar.xz`, `tar.zst`, `tar.lz4`) if the file name ends with it.
/// Cheaper than double-invoking `Path::extension`.
fn double_extension(path: &Path) -> Option<String> {
    let name = path.file_name().and_then(|s| s.to_str())?.to_ascii_lowercase();
    for &candidate in &["tar.gz", "tar.bz2", "tar.xz", "tar.zst", "tar.lz4"] {
        if name.ends_with(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_magic() {
        assert_eq!(detect_bytes(&[0x50, 0x4B, 0x03, 0x04, 0x00]), Some(ArchiveFormat::Zip));
    }

    #[test]
    fn sevenz_magic() {
        assert_eq!(
            detect_bytes(&[0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C, 0x00]),
            Some(ArchiveFormat::SevenZ)
        );
    }

    #[test]
    fn rar5_magic() {
        assert_eq!(
            detect_bytes(&[0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00]),
            Some(ArchiveFormat::Rar)
        );
    }

    #[test]
    fn gzip_magic() {
        assert_eq!(detect_bytes(&[0x1F, 0x8B, 0x08]), Some(ArchiveFormat::Gzip));
    }

    #[test]
    fn bzip2_magic() {
        assert_eq!(detect_bytes(b"BZh9"), Some(ArchiveFormat::TarBz2));
    }

    #[test]
    fn xz_magic() {
        assert_eq!(
            detect_bytes(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00, 0x00]),
            Some(ArchiveFormat::TarXz)
        );
    }

    #[test]
    fn tar_ustar_offset_257() {
        let mut buf = vec![0u8; 512];
        buf[TAR_USTAR_OFFSET..TAR_USTAR_OFFSET + 5].copy_from_slice(b"ustar");
        assert_eq!(detect_bytes(&buf), Some(ArchiveFormat::Tar));
    }

    #[test]
    fn unknown_returns_none() {
        assert_eq!(detect_bytes(&[0, 1, 2, 3]), None);
    }

    // --- Phase 7+ option Y: single-stream variants ---------------------

    #[test]
    fn ext_hint_plain_bz2_yields_bzip2_single_stream() {
        // The bare .bz2 case used to yield Unknown; v7 elevates it to Bzip2.
        assert_eq!(extension_hint(Path::new("foo.bz2")), Some(ArchiveFormat::Bzip2));
    }

    #[test]
    fn ext_hint_plain_xz_yields_xz_single_stream() {
        assert_eq!(extension_hint(Path::new("foo.xz")), Some(ArchiveFormat::Xz));
    }

    #[test]
    fn ext_hint_lzma_yields_lzma_single_stream() {
        assert_eq!(extension_hint(Path::new("foo.lzma")), Some(ArchiveFormat::Lzma));
    }

    #[test]
    fn ext_hint_tarball_still_routes_to_tar_family() {
        // Regression guard: the v7 changes must not break the .tar.* cases.
        assert_eq!(extension_hint(Path::new("foo.tar.bz2")), Some(ArchiveFormat::TarBz2));
        assert_eq!(extension_hint(Path::new("foo.tar.xz")), Some(ArchiveFormat::TarXz));
        assert_eq!(extension_hint(Path::new("foo.tbz2")), Some(ArchiveFormat::TarBz2));
        assert_eq!(extension_hint(Path::new("foo.txz")), Some(ArchiveFormat::TarXz));
    }

    #[test]
    fn upgrade_with_ext_bz2_picks_single_stream() {
        // BZh magic alone is ambiguous between plain .bz2 and .tar.bz2;
        // the .bz2 extension narrows it to Bzip2 (v7) instead of Unknown.
        let p = Path::new("payload.bz2");
        assert_eq!(upgrade_with_extension(ArchiveFormat::TarBz2, p), ArchiveFormat::Bzip2);
    }

    #[test]
    fn upgrade_with_ext_xz_picks_single_stream() {
        let p = Path::new("payload.xz");
        assert_eq!(upgrade_with_extension(ArchiveFormat::TarXz, p), ArchiveFormat::Xz);
    }

    // --- PR-F2: ZSTD / LZ4 family -------------------------------------

    #[test]
    fn zstd_magic() {
        // Just the magic; the rest of the frame header is not needed for
        // detection (RFC 8478 §3.1.1).
        assert_eq!(
            detect_bytes(&[0x28, 0xB5, 0x2F, 0xFD, 0x00]),
            Some(ArchiveFormat::Zstd)
        );
    }

    #[test]
    fn lz4_frame_magic() {
        assert_eq!(
            detect_bytes(&[0x04, 0x22, 0x4D, 0x18, 0x00]),
            Some(ArchiveFormat::Lz4)
        );
    }

    #[test]
    fn ext_hint_zst_yields_single_stream() {
        assert_eq!(extension_hint(Path::new("foo.zst")), Some(ArchiveFormat::Zstd));
    }

    #[test]
    fn ext_hint_lz4_yields_single_stream() {
        assert_eq!(extension_hint(Path::new("foo.lz4")), Some(ArchiveFormat::Lz4));
    }

    #[test]
    fn ext_hint_tar_zst_yields_tar_variant() {
        assert_eq!(extension_hint(Path::new("foo.tar.zst")), Some(ArchiveFormat::TarZst));
        assert_eq!(extension_hint(Path::new("foo.tzst")), Some(ArchiveFormat::TarZst));
    }

    #[test]
    fn ext_hint_tar_lz4_yields_tar_variant() {
        assert_eq!(extension_hint(Path::new("foo.tar.lz4")), Some(ArchiveFormat::TarLz4));
    }

    #[test]
    fn upgrade_with_ext_zstd_picks_tar_variant_for_dotted_name() {
        // Tentative Zstd from magic + .tar.zst extension -> TarZst.
        let p = Path::new("payload.tar.zst");
        assert_eq!(upgrade_with_extension(ArchiveFormat::Zstd, p), ArchiveFormat::TarZst);
    }

    #[test]
    fn upgrade_with_ext_lz4_picks_tar_variant_for_dotted_name() {
        let p = Path::new("payload.tar.lz4");
        assert_eq!(upgrade_with_extension(ArchiveFormat::Lz4, p), ArchiveFormat::TarLz4);
    }

    // --- PR-F3: ZIP variant aliases -----------------------------------

    #[test]
    fn ext_hint_jar_yields_zip() {
        assert_eq!(extension_hint(Path::new("library.jar")), Some(ArchiveFormat::Zip));
    }

    #[test]
    fn ext_hint_war_ear_yield_zip() {
        assert_eq!(extension_hint(Path::new("webapp.war")), Some(ArchiveFormat::Zip));
        assert_eq!(extension_hint(Path::new("enterprise.ear")), Some(ArchiveFormat::Zip));
    }

    #[test]
    fn ext_hint_apk_aab_ipa_yield_zip() {
        assert_eq!(extension_hint(Path::new("MyApp.apk")), Some(ArchiveFormat::Zip));
        assert_eq!(extension_hint(Path::new("MyApp.aab")), Some(ArchiveFormat::Zip));
        assert_eq!(extension_hint(Path::new("MyApp.ipa")), Some(ArchiveFormat::Zip));
    }

    #[test]
    fn ext_hint_appx_msix_yield_zip() {
        assert_eq!(extension_hint(Path::new("Pkg.appx")), Some(ArchiveFormat::Zip));
        assert_eq!(extension_hint(Path::new("Pkg.msix")), Some(ArchiveFormat::Zip));
    }

    #[test]
    fn ext_hint_xpi_crx_yield_zip() {
        assert_eq!(extension_hint(Path::new("addon.xpi")), Some(ArchiveFormat::Zip));
        assert_eq!(extension_hint(Path::new("ext.crx")), Some(ArchiveFormat::Zip));
    }

    #[test]
    fn ext_hint_uppercase_alias_still_yields_zip() {
        // lowercase_extension does the casefold; just sanity-check.
        assert_eq!(extension_hint(Path::new("CAPS.JAR")), Some(ArchiveFormat::Zip));
        assert_eq!(extension_hint(Path::new("MyApp.APK")), Some(ArchiveFormat::Zip));
    }

    // --- PR-F4: .tlz alias ---------------------------------------------

    #[test]
    fn ext_hint_tlz_collapses_to_tar_xz_slot() {
        // The detect layer cannot tell LZMA1 apart from LZMA2 by magic
        // alone (LZMA1 has no real header signature). We use the
        // `TarXz` enum slot for both; the path-aware dispatcher in
        // `backends::open_backend` selects the right codec.
        assert_eq!(extension_hint(Path::new("bundle.tlz")), Some(ArchiveFormat::TarXz));
    }
}
