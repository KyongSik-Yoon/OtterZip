//! Archive open / close / metadata FFI. See `ffi-api.md` §4-5.

use std::os::raw::c_char;

use otterzip_core::{Archive, ArchiveFormat, OpenMode, OtterzipError};
// Re-imports above already pull `ArchiveFormat` for the bytes detector.

use crate::error::{ErrorCode, OK};
use crate::util::{catch_unwind_to_error, read_optional_utf8, read_utf8};

/// Opaque archive handle exposed to C. The actual storage is a heap-allocated
/// `otterzip_core::Archive` whose pointer we hand back as `*mut OtterzipArchive`.
/// Callers must treat this as opaque — the layout is *not* part of the ABI
/// and may change without bumping `otterzip_abi_version`.
#[repr(C)]
pub struct OtterzipArchive {
    _private: [u8; 0],
}

fn open_mode_from_u32(value: u32) -> Result<OpenMode, OtterzipError> {
    match value {
        0 => Ok(OpenMode::Read),
        1 => Ok(OpenMode::CreateNew),
        2 => Ok(OpenMode::CreateOrOverwrite),
        3 => Ok(OpenMode::Update),
        _ => Err(OtterzipError::InvalidArgument("mode out of range")),
    }
}

/// Detect the archive format at `path_utf8` (magic bytes preferred, extension
/// as a tiebreak). On success writes the [`ArchiveFormat`] discriminant into
/// `out_format` and returns [`ErrorCode::Ok`].
#[no_mangle]
pub extern "C" fn otterzip_detect_format(
    path_utf8: *const c_char,
    path_len: usize,
    out_format: *mut u32,
) -> i32 {
    catch_unwind_to_error(|| {
        if out_format.is_null() {
            return Err(OtterzipError::InvalidArgument("out_format is null"));
        }
        // SAFETY: caller guarantees `(path_utf8, path_len)` is a valid slice.
        let path = unsafe { read_utf8(path_utf8, path_len)? };
        let fmt = otterzip_core::detect(path)?;
        // SAFETY: null-checked above.
        unsafe { *out_format = fmt as u32 };
        Ok(OK)
    })
}

/// Streaming-friendly variant: classify an in-memory byte prefix.
/// Mirrors `ffi-api.md` §3 `otterzip_detect_format_bytes`.
#[no_mangle]
pub extern "C" fn otterzip_detect_format_bytes(
    data: *const u8,
    len: usize,
    out_format: *mut u32,
) -> i32 {
    catch_unwind_to_error(|| {
        if out_format.is_null() {
            return Err(OtterzipError::InvalidArgument("out_format is null"));
        }
        if data.is_null() && len > 0 {
            return Err(OtterzipError::InvalidArgument("null data with non-zero len"));
        }
        // SAFETY: caller-provided slice; len validated against null above.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) };
        let fmt = otterzip_core::detect_bytes(bytes).unwrap_or(ArchiveFormat::Unknown);
        // SAFETY: out_format null-checked above.
        unsafe { *out_format = fmt as u32 };
        Ok(OK)
    })
}

/// `Archive::entry_count` mirror.
#[no_mangle]
pub extern "C" fn otterzip_archive_entry_count(
    handle: *const OtterzipArchive,
    out_count: *mut u64,
) -> i32 {
    catch_unwind_to_error(|| {
        if handle.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        if out_count.is_null() {
            return Err(OtterzipError::InvalidArgument("out_count is null"));
        }
        // SAFETY: handle from `otterzip_archive_open`.
        let archive = unsafe { &*handle.cast::<Archive>() };
        let n = archive.entry_count()?;
        // SAFETY: null-checked.
        unsafe { *out_count = n };
        Ok(OK)
    })
}

/// `Archive::is_encrypted` mirror. Writes `0` or `1` into `out_bool`.
#[no_mangle]
pub extern "C" fn otterzip_archive_is_encrypted(
    handle: *const OtterzipArchive,
    out_bool: *mut u8,
) -> i32 {
    catch_unwind_to_error(|| {
        if handle.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        if out_bool.is_null() {
            return Err(OtterzipError::InvalidArgument("out_bool is null"));
        }
        // SAFETY: handle valid per open contract.
        let archive = unsafe { &*handle.cast::<Archive>() };
        let v = archive.is_encrypted()?;
        // SAFETY: null-checked.
        unsafe { *out_bool = u8::from(v) };
        Ok(OK)
    })
}

/// `Archive::is_solid` mirror per `ffi-api.md` §5: writes `1`/`0`/`-1`
/// (yes / no / not applicable) into `out_tri`.
#[no_mangle]
pub extern "C" fn otterzip_archive_is_solid(
    handle: *const OtterzipArchive,
    out_tri: *mut i8,
) -> i32 {
    catch_unwind_to_error(|| {
        if handle.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        if out_tri.is_null() {
            return Err(OtterzipError::InvalidArgument("out_tri is null"));
        }
        // SAFETY: handle valid per open contract.
        let archive = unsafe { &*handle.cast::<Archive>() };
        let v = match archive.is_solid() {
            Some(true) => 1i8,
            Some(false) => 0i8,
            None => -1i8,
        };
        // SAFETY: null-checked.
        unsafe { *out_tri = v };
        Ok(OK)
    })
}

/// Open an existing archive in read mode. `password_utf8` may be NULL with
/// `password_len = 0`; password support itself lands in Sprint 3 and currently
/// returns [`ErrorCode::FeatureDisabled`].
///
/// On success the function writes the new handle through `out_handle` and
/// returns [`ErrorCode::Ok`]. On failure `*out_handle` is left untouched.
#[no_mangle]
pub extern "C" fn otterzip_archive_open(
    path_utf8: *const c_char,
    path_len: usize,
    mode: u32,
    password_utf8: *const c_char,
    password_len: usize,
    out_handle: *mut *mut OtterzipArchive,
) -> i32 {
    catch_unwind_to_error(|| {
        if out_handle.is_null() {
            return Err(OtterzipError::InvalidArgument("out_handle is null"));
        }
        // SAFETY: caller-provided slices, validated by `read_utf8`.
        let path = unsafe { read_utf8(path_utf8, path_len)? };
        let password = unsafe { read_optional_utf8(password_utf8, password_len)? };
        let mode = open_mode_from_u32(mode)?;

        let archive = match password {
            Some(p) => Archive::open_with_password(path, mode, p.to_owned())?,
            None => Archive::open(path, mode)?,
        };

        let boxed = Box::new(archive);
        // SAFETY: out_handle null-checked above. The boxed Archive is now
        // owned by C; `otterzip_archive_close` must be called to release.
        unsafe {
            *out_handle = Box::into_raw(boxed).cast::<OtterzipArchive>();
        }
        Ok(OK)
    })
}

/// Close a previously-opened archive. Accepts NULL as a no-op so callers can
/// always defer-free without branching.
#[no_mangle]
pub extern "C" fn otterzip_archive_close(handle: *mut OtterzipArchive) {
    if handle.is_null() {
        return;
    }
    // SAFETY: handle came from `otterzip_archive_open`, which boxed an
    // `Archive`. We reverse the cast and drop. C contract: caller must not
    // use `handle` after this call; we don't observe that here.
    unsafe {
        drop(Box::from_raw(handle.cast::<Archive>()));
    }
}

/// Read the detected archive format. Returns [`ErrorCode::InvalidHandle`]
/// if `handle` is NULL.
#[no_mangle]
pub extern "C" fn otterzip_archive_format(
    handle: *const OtterzipArchive,
    out_format: *mut u32,
) -> i32 {
    catch_unwind_to_error(|| {
        if handle.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        if out_format.is_null() {
            return Err(OtterzipError::InvalidArgument("out_format is null"));
        }
        // SAFETY: handle is a valid `*const Archive` per the open contract.
        let archive = unsafe { &*handle.cast::<Archive>() };
        // SAFETY: null-checked above.
        unsafe { *out_format = archive.format() as u32 };
        Ok(OK)
    })
}

/// Internal: borrow the inner [`Archive`] for the lifetime of `handle`. The
/// returned reference is only valid while `handle` outlives it.
///
/// # Safety
/// `handle` must be a non-null pointer originally produced by
/// [`otterzip_archive_open`] (or a future create/update equivalent), and the
/// caller must enforce single-threaded access per `ffi-api.md` §0.1 #7.
pub(crate) unsafe fn archive_ref<'a>(handle: *const OtterzipArchive) -> Option<&'a Archive> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: function-level contract.
    Some(unsafe { &*handle.cast::<Archive>() })
}

/// Format-discriminant helper used by tests.
#[doc(hidden)]
#[must_use]
pub const fn format_discriminant(f: ArchiveFormat) -> u32 {
    f as u32
}
