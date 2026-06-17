//! Archive open / close / metadata FFI. See `ffi-api.md` §4-5.

use std::os::raw::c_char;

use otterzip_core::{Archive, ArchiveFormat, OpenMode, OtterzipError, RootLayout};
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
        let t0 = std::time::Instant::now();
        // SAFETY: handle valid per open contract.
        let archive = unsafe { &*handle.cast::<Archive>() };
        let v = archive.is_encrypted()?;
        tracing::info!(
            target: "otterzip::ffi",
            elapsed_ms = t0.elapsed().as_millis() as u64,
            is_encrypted = v,
            "FFI archive_is_encrypted done"
        );
        // SAFETY: null-checked.
        unsafe { *out_bool = u8::from(v) };
        Ok(OK)
    })
}

/// `Archive::detect_root_layout` mirror.
///
/// Probes the archive's top-level layout for Smart Extract decisions
/// (Bandizip "알아서 풀기" parity).
///
/// Outputs:
///   * `out_is_single_folder`: `0` for [`RootLayout::Flat`], `1` for
///     [`RootLayout::SingleFolder`]. Always written on success.
///   * `out_name_utf8` + `name_capacity`: caller-allocated buffer
///     receives the SingleFolder name (NUL-terminated). On Flat the
///     buffer is zeroed.
///   * `out_name_len`: written length (excluding the NUL). 0 on Flat.
///
/// If the folder name doesn't fit in `name_capacity`, returns
/// [`ErrorCode::InvalidArgument`] with `out_name_len` set to the
/// required size (excluding NUL) so callers can retry with a larger
/// buffer. 256 chars is sufficient for every Windows path component
/// (MAX_PATH = 260 including drive and separator).
#[no_mangle]
pub extern "C" fn otterzip_archive_detect_root_layout(
    handle: *const OtterzipArchive,
    out_is_single_folder: *mut i32,
    out_name_utf8: *mut c_char,
    name_capacity: usize,
    out_name_len: *mut usize,
) -> i32 {
    catch_unwind_to_error(|| {
        if handle.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        if out_is_single_folder.is_null()
            || out_name_utf8.is_null()
            || out_name_len.is_null()
        {
            return Err(OtterzipError::InvalidArgument(
                "out parameter is null",
            ));
        }
        // SAFETY: handle valid per open contract.
        let archive = unsafe { &*handle.cast::<Archive>() };
        let layout = archive.detect_root_layout()?;

        match layout {
            RootLayout::Flat => {
                // SAFETY: null-checked above.
                unsafe {
                    *out_is_single_folder = 0;
                    *out_name_len = 0;
                    // Zero the buffer so callers reading without
                    // checking out_name_len don't get stale data.
                    if name_capacity > 0 {
                        *out_name_utf8 = 0;
                    }
                }
            }
            RootLayout::SingleFolder(name) => {
                let bytes = name.as_bytes();
                let required = bytes.len();
                // SAFETY: null-checked above.
                unsafe {
                    *out_is_single_folder = 1;
                    *out_name_len = required;
                }
                // Need room for the bytes plus one NUL.
                if required + 1 > name_capacity {
                    return Err(OtterzipError::InvalidArgument(
                        "out_name_utf8 capacity too small",
                    ));
                }
                // SAFETY: buffer is at least `required + 1` bytes
                // (just verified), and `bytes` is a valid UTF-8 slice.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        out_name_utf8.cast::<u8>(),
                        required,
                    );
                    *out_name_utf8.add(required) = 0;
                }
            }
        }
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
        let t0 = std::time::Instant::now();
        let file_size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        tracing::info!(
            target: "otterzip::ffi",
            path,
            has_password = password.is_some(),
            file_size_bytes,
            "FFI archive_open begin"
        );

        let archive = match password {
            Some(p) => Archive::open_with_password(path, mode, p.to_owned())?,
            None => Archive::open(path, mode)?,
        };
        tracing::info!(
            target: "otterzip::ffi",
            path,
            elapsed_ms = t0.elapsed().as_millis() as u64,
            format = ?archive.format(),
            "FFI archive_open done"
        );

        let boxed = Box::new(archive);
        // SAFETY: out_handle null-checked above. The boxed Archive is now
        // owned by C; `otterzip_archive_close` must be called to release.
        unsafe {
            *out_handle = Box::into_raw(boxed).cast::<OtterzipArchive>();
        }
        Ok(OK)
    })
}

/// Open a split / spanned archive given an ordered list of volume
/// paths. Layout (per `schema.md` §4 UTF-8 + explicit-length rule):
///
///   * `packed_paths_utf8` — one packed UTF-8 buffer holding every
///     path's bytes back-to-back (no separators, no null terminators).
///   * `packed_paths_len` — total byte length of that buffer.
///   * `path_offsets` — array of `path_count` byte offsets into the
///     packed buffer marking each path's start.
///   * `path_lengths` — parallel array of `path_count` lengths.
///
/// On success `out_handle` receives a fresh archive handle to be
/// released via `otterzip_archive_close`. The volumes must be supplied
/// in disk order (volume 1 → last).
///
/// ABI v8: added.
#[no_mangle]
pub extern "C" fn otterzip_archive_open_multi(
    packed_paths_utf8: *const u8,
    packed_paths_len: usize,
    path_offsets: *const usize,
    path_lengths: *const usize,
    path_count: usize,
    mode: u32,
    out_handle: *mut *mut OtterzipArchive,
) -> i32 {
    catch_unwind_to_error(|| {
        if out_handle.is_null() {
            return Err(OtterzipError::InvalidArgument("out_handle is null"));
        }
        if path_count == 0 {
            return Err(OtterzipError::InvalidArgument(
                "open_multi requires path_count > 0",
            ));
        }
        if packed_paths_utf8.is_null() || path_offsets.is_null() || path_lengths.is_null() {
            return Err(OtterzipError::InvalidArgument(
                "open_multi: packed buffer / offset / length pointer is null",
            ));
        }

        // SAFETY: caller contract — pointers reference buffers sized by
        // `packed_paths_len` and `path_count` respectively.
        let packed = unsafe { std::slice::from_raw_parts(packed_paths_utf8, packed_paths_len) };
        let offsets = unsafe { std::slice::from_raw_parts(path_offsets, path_count) };
        let lengths = unsafe { std::slice::from_raw_parts(path_lengths, path_count) };

        let mut volumes: Vec<std::path::PathBuf> = Vec::with_capacity(path_count);
        for i in 0..path_count {
            let off = offsets[i];
            let len = lengths[i];
            let end = off
                .checked_add(len)
                .ok_or(OtterzipError::InvalidArgument("path offset+length overflow"))?;
            if end > packed.len() {
                return Err(OtterzipError::InvalidArgument(
                    "path slice extends past packed buffer end",
                ));
            }
            let s = std::str::from_utf8(&packed[off..end])
                .map_err(|_| OtterzipError::InvalidArgument("path is not valid UTF-8"))?;
            volumes.push(std::path::PathBuf::from(s));
        }
        let mode = open_mode_from_u32(mode)?;
        let archive = Archive::open_multi(&volumes, mode)?;
        let boxed = Box::new(archive);
        // SAFETY: out_handle null-checked above.
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
