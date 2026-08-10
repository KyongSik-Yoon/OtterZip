//! POSIX file-metadata restoration — the Unix half of the platform layer.
//!
//! Windows keeps its per-file security in ACLs that no archive format
//! records, so the Windows build has never had anything to restore. On
//! Linux and macOS the permission bits ARE in the archive, and dropping
//! them is a visible data loss: a `.tar.gz` of a build tree extracts with
//! every script at `rw-------`-ish defaults and `./configure` fails with
//! "Permission denied". This module is the counterpart to
//! [`crate::motw`] — that one is a no-op off Windows, this one is a no-op
//! on Windows.
//!
//! Two jobs:
//!   * [`apply_mode`] — map an [`Entry`](crate::Entry)'s `attributes` word
//!     onto the extracted file's mode.
//!   * [`create_symlink`] — materialise a symlink entry with the target
//!     validated to stay inside the destination root.

#[cfg(unix)]
use std::path::Path;

use crate::entry::HostOs;

/// Recover the POSIX permission bits from an [`Entry`](crate::Entry)'s
/// `attributes` word, or `None` when the archive carries no Unix metadata
/// for this entry.
///
/// `attributes` is a raw passthrough of whatever the format stored, and the
/// three shapes in the wild are not distinguishable by `host_os` alone —
/// most backends report [`HostOs::Unknown`] because their crates do not
/// surface the creator-OS byte. So the shape is inferred from the value:
///
/// | shape | producer | layout |
/// |---|---|---|
/// | A | ZIP / 7z external attributes | `st_mode` in the **high** 16 bits |
/// | B | RAR (`file_attr`), tar with type bits | `st_mode` in the low bits |
/// | C | tar (`header.mode()`) | permission bits only, no `S_IFMT` |
///
/// Shapes A and B are self-identifying: they carry a non-zero `S_IFMT`
/// nibble (`0o170000`), which a Windows `FILE_ATTRIBUTE_*` word never has
/// — those top out at `0x8000` (`FILE_ATTRIBUTE_INTEGRITY_STREAM`) and the
/// common values (`ARCHIVE` `0x20`, `DIRECTORY` `0x10`, `READONLY` `0x1`)
/// are nowhere near it. Shape C has no marker at all, so it is accepted
/// only when the backend positively reports [`HostOs::Unix`].
#[must_use]
pub fn unix_mode_from_attributes(attributes: u32, host_os: HostOs) -> Option<u32> {
    const S_IFMT: u32 = 0o170_000;
    const PERM_BITS: u32 = 0o7777;

    // Shape A — ZIP / 7z external file attributes.
    let high = attributes >> 16;
    if high & S_IFMT != 0 {
        return Some(high & PERM_BITS);
    }
    // Shape B — st_mode stored directly (RAR from a Unix host, tar headers
    // that kept their type bits).
    if attributes & S_IFMT != 0 {
        return Some(attributes & PERM_BITS);
    }
    // Shape C — bare permission bits. No marker, so require the backend to
    // vouch for the origin; otherwise a Windows `FILE_ATTRIBUTE_ARCHIVE`
    // (0x20) would be read as mode 0o40 and strip the owner's read bit.
    if host_os == HostOs::Unix && attributes != 0 && attributes & !PERM_BITS == 0 {
        return Some(attributes & PERM_BITS);
    }
    None
}

/// This process's umask, read once and cached.
///
/// There is no portable read-only accessor: the POSIX `umask()` call is a
/// swap, so reading it means `umask(umask(0))` — a window in which every
/// other thread in the process sees a zero umask. That is unacceptable in a
/// library, so on Linux we read the value out of `/proc/self/status`
/// (`Umask:` line, present since Linux 4.7) instead. Anywhere else, and if
/// procfs is unavailable, fall back to the near-universal default `0o022`.
#[cfg(unix)]
fn process_umask() -> u32 {
    use std::sync::OnceLock;
    static UMASK: OnceLock<u32> = OnceLock::new();

    *UMASK.get_or_init(|| {
        #[cfg(target_os = "linux")]
        {
            if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                for line in status.lines() {
                    if let Some(v) = line.strip_prefix("Umask:") {
                        if let Ok(m) = u32::from_str_radix(v.trim(), 8) {
                            return m;
                        }
                    }
                }
            }
        }
        0o022
    })
}

/// Apply an archived entry's permission bits to a freshly extracted file.
///
/// No-op on Windows, when `preserve_permissions` is off, or when the entry
/// carries no recoverable Unix mode.
///
/// Two deliberate narrowings of what the archive asked for:
///
/// * **`setuid` / `setgid` / sticky are dropped** (`& 0o777`). An archive is
///   attacker-controlled input; honouring a `04755` entry would let any
///   downloaded `.tar.gz` drop a setuid-root binary into a directory the
///   user later runs from. GNU tar only restores those bits under explicit
///   `-p` as root, and no desktop extract flow qualifies.
/// * **The umask is applied**, matching `tar`/`unzip` for a non-root user.
///   Without it a `0o777` entry would produce a world-writable file on a
///   machine whose umask says otherwise.
///
/// The owner's read+write bits are then forced back on: a mode like `0o444`
/// (or `0o000`, which real archives do contain) would otherwise leave the
/// user unable to delete or edit what they just extracted, and unable to let
/// the extractor itself clean up on rollback.
///
/// Best-effort by design — a failure here (foreign filesystem, no `chmod`
/// support on the mount) must not fail an otherwise complete extraction, so
/// the result is discarded and the caller keeps going.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn apply_mode(file: &std::fs::File, attributes: u32, host_os: HostOs, preserve: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if !preserve {
            return;
        }
        let Some(mode) = unix_mode_from_attributes(attributes, host_os) else {
            return;
        };
        let effective = ((mode & 0o777) & !process_umask()) | 0o600;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(effective));
    }
}

/// Directory variant of [`apply_mode`] — same rules, but addressed by path
/// because directories are created with `create_dir_all` rather than an open
/// handle.
///
/// The owner's `rwx` is forced on rather than just `rw`: a directory the
/// extractor cannot enter would abort the extraction of everything beneath
/// it, and a `0o444` directory entry is not rare in archives built by
/// permission-mangling tooling.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn apply_dir_mode(path: &std::path::Path, attributes: u32, host_os: HostOs, preserve: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if !preserve {
            return;
        }
        let Some(mode) = unix_mode_from_attributes(attributes, host_os) else {
            return;
        };
        let effective = ((mode & 0o777) & !process_umask()) | 0o700;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(effective));
    }
}

/// Why a symlink entry could not be materialised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkRejected {
    /// Not a Unix host — symlink creation is not attempted at all.
    UnsupportedPlatform,
    /// Empty target, or one that is not representable as a path.
    EmptyTarget,
    /// The target is absolute (`/etc/shadow`) — always out of the
    /// destination root by construction.
    AbsoluteTarget,
    /// The target resolves outside the destination root via `..`.
    EscapesRoot,
}

/// Resolve a symlink target lexically against the link's own directory and
/// confirm it stays inside `dest_root`.
///
/// Purely lexical on purpose: the target frequently does not exist yet (tar
/// streams links before their referents), so `canonicalize` is not
/// available. `dest_root` is already canonical at every call site, and no
/// component of the link's own path can be a symlink — every directory on
/// the way was created by this same extraction, which never follows an
/// existing link. So the lexical answer and the resolved answer agree.
///
/// Returns the target exactly as it should be written into the link (the
/// original relative string), not the resolved path — a symlink's stored
/// value is meant to stay relative so the extracted tree can be moved.
#[cfg(unix)]
fn validate_symlink_target<'t>(
    dest_root: &Path,
    link_path: &Path,
    target: &'t str,
) -> Result<&'t str, SymlinkRejected> {
    if target.is_empty() {
        return Err(SymlinkRejected::EmptyTarget);
    }
    if target.starts_with('/') || target.starts_with('\\') {
        return Err(SymlinkRejected::AbsoluteTarget);
    }

    // Walk the target's segments against the link's parent directory,
    // popping on `..`. Anything that would pop above `dest_root` escapes.
    let base = link_path.parent().unwrap_or(dest_root);
    let mut depth: isize = base
        .strip_prefix(dest_root)
        .map(|rel| rel.components().count())
        .map_err(|_| SymlinkRejected::EscapesRoot)?
        .try_into()
        .unwrap_or(isize::MAX);

    for seg in target.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return Err(SymlinkRejected::EscapesRoot);
                }
            }
            _ => depth += 1,
        }
    }
    Ok(target)
}

/// Materialise a symlink entry at `link_path` pointing at `target`.
///
/// Called only when the caller has opted in (`ExtractOptions::follow_symlinks`);
/// the default remains "skip symlink entries entirely", which is what the
/// Windows build does for want of an unprivileged `CreateSymbolicLinkW`.
///
/// The target is validated against `dest_root` first, so a `../../.ssh/authorized_keys`
/// link cannot be used to write outside the destination on the next entry —
/// the classic two-entry tar escape (link, then a regular file whose path
/// traverses it). Note the traversal guard on entry *paths* does not cover
/// that attack: both entries look contained, and it is the link in between
/// that redirects the write.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn create_symlink(
    dest_root: &std::path::Path,
    link_path: &std::path::Path,
    target: &str,
) -> Result<(), SymlinkRejected> {
    #[cfg(unix)]
    {
        let target = validate_symlink_target(dest_root, link_path, target)?;
        // An existing entry at the slot (overwrite policies resolve to the
        // original path) has to go first — `symlink` fails on any existing
        // name, and `remove_file` also unlinks a stale symlink without
        // following it.
        let _ = std::fs::remove_file(link_path);
        std::os::unix::fs::symlink(target, link_path).map_err(|_| SymlinkRejected::EmptyTarget)
    }
    #[cfg(not(unix))]
    {
        Err(SymlinkRejected::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_external_attributes_decode_from_the_high_word() {
        // 0o100755 regular executable, ZIP external-attribute layout.
        let attrs = 0o100_755 << 16;
        assert_eq!(
            unix_mode_from_attributes(attrs, HostOs::Unknown),
            Some(0o755)
        );
    }

    #[test]
    fn rar_unix_file_attr_decodes_in_place() {
        // 0x81a4 == 0o100644, what unrar reports for a Unix-authored entry.
        assert_eq!(
            unix_mode_from_attributes(0x81a4, HostOs::Unknown),
            Some(0o644)
        );
    }

    #[test]
    fn bare_tar_mode_needs_a_unix_host_vouch() {
        assert_eq!(unix_mode_from_attributes(0o755, HostOs::Unix), Some(0o755));
        assert_eq!(unix_mode_from_attributes(0o755, HostOs::Unknown), None);
    }

    #[test]
    fn windows_file_attributes_are_not_mistaken_for_a_mode() {
        // FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_READONLY, the pair almost
        // every Windows-authored 7z / CAB entry carries.
        assert_eq!(unix_mode_from_attributes(0x21, HostOs::Windows), None);
        assert_eq!(unix_mode_from_attributes(0x21, HostOs::Unknown), None);
        // Directory bit, likewise.
        assert_eq!(unix_mode_from_attributes(0x10, HostOs::Unknown), None);
    }

    #[test]
    fn no_metadata_yields_none() {
        assert_eq!(unix_mode_from_attributes(0, HostOs::Unix), None);
        assert_eq!(unix_mode_from_attributes(0, HostOs::Unknown), None);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_targets_are_confined_to_the_destination_root() {
        let root = Path::new("/tmp/dest");

        // Sibling inside the root.
        assert_eq!(
            validate_symlink_target(root, Path::new("/tmp/dest/a/link"), "b.txt"),
            Ok("b.txt")
        );
        // Climbing back to the root and down again is fine.
        assert_eq!(
            validate_symlink_target(root, Path::new("/tmp/dest/a/link"), "../b/c.txt"),
            Ok("../b/c.txt")
        );
        // One level too far.
        assert_eq!(
            validate_symlink_target(root, Path::new("/tmp/dest/a/link"), "../../escape"),
            Err(SymlinkRejected::EscapesRoot)
        );
        // A link directly under the root cannot climb at all.
        assert_eq!(
            validate_symlink_target(root, Path::new("/tmp/dest/link"), "../escape"),
            Err(SymlinkRejected::EscapesRoot)
        );
        // Absolute targets never qualify.
        assert_eq!(
            validate_symlink_target(root, Path::new("/tmp/dest/link"), "/etc/shadow"),
            Err(SymlinkRejected::AbsoluteTarget)
        );
        assert_eq!(
            validate_symlink_target(root, Path::new("/tmp/dest/link"), ""),
            Err(SymlinkRejected::EmptyTarget)
        );
    }

    #[cfg(unix)]
    #[test]
    fn backslash_separated_targets_are_walked_too() {
        let root = Path::new("/tmp/dest");
        // A Windows-authored archive can store `..\..\escape`; splitting on
        // `/` alone would see one segment and count it as a descent.
        assert_eq!(
            validate_symlink_target(root, Path::new("/tmp/dest/a/link"), "..\\..\\escape"),
            Err(SymlinkRejected::EscapesRoot)
        );
    }
}
