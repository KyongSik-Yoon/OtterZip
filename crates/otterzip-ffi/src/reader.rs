//! Streaming reader FFI surface (`ffi-api.md` §7).
//!
//! `otterzip_reader_open` returns an opaque `OtterzipReader*` that wraps
//! the result of `Archive::read_entry`. `otterzip_reader_read` pulls
//! decompressed bytes into a caller-owned buffer until EOF.
//!
//! Lifetime: the reader borrows the archive in the Rust API. We erase
//! that lifetime to `'static` here — the C contract requires the caller
//! to keep the parent `OtterzipArchive*` alive at least as long as the
//! reader, mirroring the iterator policy in `iterator.rs`.

use std::os::raw::c_char;

use otterzip_core::OtterzipError;

use crate::archive::{archive_ref, OtterzipArchive};
use crate::error::{ErrorCode, OK};
use crate::util::{catch_unwind_to_error, read_utf8};

/// Opaque streaming reader.
#[repr(C)]
pub struct OtterzipReader {
    _private: [u8; 0],
}

struct ReaderState {
    inner: Box<dyn std::io::Read + Send + 'static>,
}

#[no_mangle]
pub extern "C" fn otterzip_reader_open(
    handle: *mut OtterzipArchive,
    entry_path_utf8: *const c_char,
    path_len: usize,
    out_reader: *mut *mut OtterzipReader,
) -> i32 {
    catch_unwind_to_error(|| {
        if out_reader.is_null() {
            return Err(OtterzipError::InvalidArgument("out_reader is null"));
        }
        // SAFETY: handle from `otterzip_archive_open`. archive_ref does null check.
        let archive = match unsafe { archive_ref(handle.cast_const()) } {
            Some(a) => a,
            None => return Ok(ErrorCode::InvalidHandle as i32),
        };
        // SAFETY: caller-provided UTF-8 slice.
        let entry = unsafe { read_utf8(entry_path_utf8, path_len)? };

        let reader = archive.read_entry(entry)?;
        // SAFETY: erase the lifetime from `'a` (= archive) to `'static`. The
        // C contract requires the caller to keep `handle` alive until
        // `otterzip_reader_close` returns; this matches the iterator
        // pattern documented in `iterator.rs`.
        let static_reader: Box<dyn std::io::Read + Send + 'static> = unsafe {
            std::mem::transmute::<
                Box<dyn std::io::Read + Send + '_>,
                Box<dyn std::io::Read + Send + 'static>,
            >(reader)
        };
        let state = Box::new(ReaderState {
            inner: static_reader,
        });
        // SAFETY: out_reader null-checked above.
        unsafe { *out_reader = Box::into_raw(state).cast::<OtterzipReader>() };
        Ok(OK)
    })
}

/// Read up to `buffer_len` bytes into `buffer`. Returns the number of
/// bytes read (>= 0) on success, `0` on EOF, or a negative error code.
#[no_mangle]
pub extern "C" fn otterzip_reader_read(
    reader: *mut OtterzipReader,
    buffer: *mut u8,
    buffer_len: usize,
) -> i64 {
    let result = catch_unwind_to_error(|| {
        if reader.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        if buffer.is_null() && buffer_len > 0 {
            return Err(OtterzipError::InvalidArgument(
                "null buffer with non-zero len",
            ));
        }
        // SAFETY: caller-provided buffer.
        let slice = unsafe { std::slice::from_raw_parts_mut(buffer, buffer_len) };
        // SAFETY: reader from `otterzip_reader_open`.
        let state = unsafe { &mut *reader.cast::<ReaderState>() };
        match state.inner.read(slice) {
            Ok(n) => Ok(i32::try_from(n).unwrap_or(i32::MAX)),
            Err(e) => Err(OtterzipError::Io(e)),
        }
    });
    i64::from(result)
}

#[no_mangle]
pub extern "C" fn otterzip_reader_close(reader: *mut OtterzipReader) {
    if reader.is_null() {
        return;
    }
    // SAFETY: came from `otterzip_reader_open`.
    unsafe {
        drop(Box::from_raw(reader.cast::<ReaderState>()));
    }
}
