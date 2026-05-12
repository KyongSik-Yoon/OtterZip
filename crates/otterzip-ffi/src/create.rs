//! Create / Update FFI surface. See `ffi-api.md` §9.

use std::ffi::c_void;
use std::os::raw::c_char;
use std::path::PathBuf;

use otterzip_core::format::{ArchiveFormat, CompressionMethod, EncryptionMethod};
use otterzip_core::{Archive, CreateOptions, OtterzipError};
use zeroize::Zeroizing;

use crate::archive::OtterzipArchive;
use crate::error::{ErrorCode, OK};
use crate::extract::{CallbackSink, OtterzipProgressCb, OtterzipProgressView};
use crate::util::{catch_unwind_to_error, read_optional_utf8, read_utf8};
// OtterzipProgressView is unused at module level (only referenced inside
// extract::CallbackSink); the explicit import documents the C ABI tie.
#[allow(unused_imports)]
use OtterzipProgressView as _UsedFromExtract;

/// Mirror of the C `OtterzipCreateOptions` struct (ffi-api.md §9.1).
///
/// **ABI v6 (2026-04-30):** trailing `exclude_system_metadata: u8` appended
/// per [docs/02-design/settings-catalog.md](../../docs/02-design/settings-catalog.md)
/// IN-MVP item 9. Older v5 callers reading this struct by value will
/// effectively get `0` for the new field (uninitialised tail), which means
/// "include everything" — same as the previous unconditional behavior, so
/// no behavioral regression for callers that don't update.
#[repr(C)]
pub struct OtterzipCreateOptions {
    pub format: u32,
    pub compression: u32,
    pub compression_level: u8,
    pub solid: u8,
    pub preserve_permissions: u8,
    pub preserve_timestamps: u8,
    pub encryption: u32,
    pub password_utf8: *const c_char,
    pub password_len: usize,
    pub volume_size_bytes: u64,
    pub thread_count: u32,
    pub exclude_system_metadata: u8,
}

fn format_from_u32(v: u32) -> Result<ArchiveFormat, OtterzipError> {
    match v {
        0 => Ok(ArchiveFormat::Unknown),
        1 => Ok(ArchiveFormat::Zip),
        2 => Ok(ArchiveFormat::SevenZ),
        3 => Ok(ArchiveFormat::Rar),
        4 => Ok(ArchiveFormat::Tar),
        5 => Ok(ArchiveFormat::Gzip),
        6 => Ok(ArchiveFormat::TarGz),
        7 => Ok(ArchiveFormat::TarBz2),
        8 => Ok(ArchiveFormat::TarXz),
        _ => Err(OtterzipError::InvalidArgument("format out of range")),
    }
}

fn compression_from_u32(v: u32) -> Result<CompressionMethod, OtterzipError> {
    match v {
        0 => Ok(CompressionMethod::Store),
        1 => Ok(CompressionMethod::Deflate),
        2 => Ok(CompressionMethod::Deflate64),
        3 => Ok(CompressionMethod::Bzip2),
        4 => Ok(CompressionMethod::Lzma),
        5 => Ok(CompressionMethod::Lzma2),
        6 => Ok(CompressionMethod::Zstd),
        7 => Ok(CompressionMethod::Ppmd),
        0xFFFF_FFFF => Ok(CompressionMethod::Unknown),
        _ => Err(OtterzipError::InvalidArgument("compression out of range")),
    }
}

fn encryption_from_u32(v: u32) -> Result<EncryptionMethod, OtterzipError> {
    match v {
        0 => Ok(EncryptionMethod::None),
        1 => Ok(EncryptionMethod::ZipCrypto),
        2 => Ok(EncryptionMethod::Aes128),
        3 => Ok(EncryptionMethod::Aes256),
        _ => Err(OtterzipError::InvalidArgument("encryption out of range")),
    }
}

/// Open a write-mode archive at `path_utf8`. Returns a handle that must be
/// finalised with [`otterzip_archive_commit`] (or discarded with
/// [`otterzip_archive_rollback`]).
#[no_mangle]
pub extern "C" fn otterzip_archive_create(
    path_utf8: *const c_char,
    path_len: usize,
    opts: *const OtterzipCreateOptions,
    out_handle: *mut *mut OtterzipArchive,
) -> i32 {
    catch_unwind_to_error(|| {
        if out_handle.is_null() {
            return Err(OtterzipError::InvalidArgument("out_handle is null"));
        }
        if opts.is_null() {
            return Err(OtterzipError::InvalidArgument("opts is null"));
        }
        // SAFETY: caller provides a valid path slice.
        let path = unsafe { read_utf8(path_utf8, path_len)? };
        // SAFETY: opts null-checked above.
        let opts_c = unsafe { &*opts };
        let password = unsafe { read_optional_utf8(opts_c.password_utf8, opts_c.password_len)? };

        let core_opts = CreateOptions {
            format: format_from_u32(opts_c.format)?,
            compression: compression_from_u32(opts_c.compression)?,
            compression_level: opts_c.compression_level,
            encryption: encryption_from_u32(opts_c.encryption)?,
            password: password.map(|p| Zeroizing::new(p.to_owned())),
            volume_size_bytes: if opts_c.volume_size_bytes == 0 {
                None
            } else {
                Some(opts_c.volume_size_bytes)
            },
            solid: opts_c.solid != 0,
            preserve_permissions: opts_c.preserve_permissions != 0,
            preserve_timestamps: opts_c.preserve_timestamps != 0,
            thread_count: if opts_c.thread_count == 0 {
                None
            } else {
                Some(opts_c.thread_count)
            },
            exclude_system_metadata: opts_c.exclude_system_metadata != 0,
        };

        let archive = Archive::create(PathBuf::from(path), core_opts)?;
        let boxed = Box::new(archive);
        // SAFETY: out_handle null-checked above.
        unsafe {
            *out_handle = Box::into_raw(boxed).cast::<OtterzipArchive>();
        }
        Ok(OK)
    })
}

/// Append a single file to a write-mode archive.
#[no_mangle]
pub extern "C" fn otterzip_archive_add_file(
    handle: *mut OtterzipArchive,
    src_path_utf8: *const c_char,
    src_path_len: usize,
    entry_path_utf8: *const c_char,
    entry_path_len: usize,
) -> i32 {
    catch_unwind_to_error(|| {
        if handle.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        // SAFETY: forwarded.
        let src = unsafe { read_utf8(src_path_utf8, src_path_len)? };
        let entry = unsafe { read_utf8(entry_path_utf8, entry_path_len)? };
        // SAFETY: handle from `otterzip_archive_create`. Single-threaded use.
        let archive = unsafe { &mut *handle.cast::<Archive>() };
        archive.add_file(PathBuf::from(src), entry)?;
        Ok(OK)
    })
}

/// Append a directory recursively. `entry_prefix_utf8` may be NULL/empty.
#[no_mangle]
pub extern "C" fn otterzip_archive_add_directory(
    handle: *mut OtterzipArchive,
    src_dir_utf8: *const c_char,
    src_dir_len: usize,
    entry_prefix_utf8: *const c_char,
    entry_prefix_len: usize,
) -> i32 {
    catch_unwind_to_error(|| {
        if handle.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        // SAFETY: forwarded.
        let src = unsafe { read_utf8(src_dir_utf8, src_dir_len)? };
        let prefix =
            unsafe { read_optional_utf8(entry_prefix_utf8, entry_prefix_len)? }.unwrap_or("");
        // SAFETY: handle from `otterzip_archive_create`.
        let archive = unsafe { &mut *handle.cast::<Archive>() };
        archive.add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(
            PathBuf::from(src),
            prefix,
            None,
        )?;
        Ok(OK)
    })
}

/// Like [`otterzip_archive_add_directory`] but with a progress + cancel
/// callback that fires once per pre-scan and again after every file
/// written. The callback returns `0` to continue or non-zero to abort,
/// matching the extract-side convention (`ffi-api.md` §8.2).
///
/// ABI v7 addition — older callers continue to call the no-callback
/// variant. The handle and string arguments stay binary-compatible.
#[no_mangle]
pub extern "C" fn otterzip_archive_add_directory_p(
    handle: *mut OtterzipArchive,
    src_dir_utf8: *const c_char,
    src_dir_len: usize,
    entry_prefix_utf8: *const c_char,
    entry_prefix_len: usize,
    progress_cb: OtterzipProgressCb,
    user_data: *mut c_void,
) -> i32 {
    catch_unwind_to_error(|| {
        if handle.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        // SAFETY: forwarded.
        let src = unsafe { read_utf8(src_dir_utf8, src_dir_len)? };
        let prefix =
            unsafe { read_optional_utf8(entry_prefix_utf8, entry_prefix_len)? }.unwrap_or("");
        // SAFETY: handle from `otterzip_archive_create`.
        let archive = unsafe { &mut *handle.cast::<Archive>() };

        match progress_cb {
            Some(cb) => {
                let mut sink = CallbackSink::new(cb, user_data);
                archive.add_dir_recursive(
                    PathBuf::from(src),
                    prefix,
                    Some(&mut sink),
                )?;
            }
            None => {
                archive.add_dir_recursive::<fn(&otterzip_core::Progress) -> bool>(
                    PathBuf::from(src),
                    prefix,
                    None,
                )?;
            }
        }
        Ok(OK)
    })
}

/// Finalise the archive on disk. Consumes the handle — caller must not
/// reuse it (and must not call [`crate::archive::otterzip_archive_close`] on
/// it afterwards).
#[no_mangle]
pub extern "C" fn otterzip_archive_commit(handle: *mut OtterzipArchive) -> i32 {
    catch_unwind_to_error(|| {
        if handle.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        // SAFETY: handle from `otterzip_archive_create`. We `Box::from_raw`
        // to reclaim ownership before consuming.
        let archive = unsafe { Box::from_raw(handle.cast::<Archive>()) };
        archive.commit()?;
        Ok(OK)
    })
}

/// Discard an in-progress write archive and remove the partial file.
#[no_mangle]
pub extern "C" fn otterzip_archive_rollback(handle: *mut OtterzipArchive) -> i32 {
    catch_unwind_to_error(|| {
        if handle.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        // SAFETY: ditto.
        let archive = unsafe { Box::from_raw(handle.cast::<Archive>()) };
        archive.rollback()?;
        Ok(OK)
    })
}
