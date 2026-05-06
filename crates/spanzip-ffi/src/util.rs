//! FFI helpers — panic guard, UTF-8 decode, error → code mapping.
//!
//! Every `extern "C"` function in this crate must funnel through
//! [`catch_unwind_to_error`] so a Rust panic never unwinds across the C ABI
//! boundary (UB per the Rustonomicon). See `ffi-api.md` §12.1.

use std::any::Any;
use std::os::raw::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

use spanzip_core::SpanzipError;

use crate::error::{set_last_error, ErrorCode};

/// Run `f` inside `catch_unwind`, mapping panics and `SpanzipError` to the
/// appropriate [`ErrorCode`]. The closure returns its own success code on
/// the happy path so callers can distinguish e.g. iterator-end from OK.
pub(crate) fn catch_unwind_to_error<F>(f: F) -> i32
where
    F: FnOnce() -> Result<i32, SpanzipError>,
{
    // AssertUnwindSafe: closures touching FFI-owned pointers don't expose
    // observable broken invariants — on panic we abort the operation and
    // surface a generic error code. UnwindSafe would force the caller to
    // wrap every captured `&mut` for no real safety win at this boundary.
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(code)) => code,
        Ok(Err(err)) => {
            let code = error_code_for(&err) as i32;
            set_last_error(err.to_string());
            code
        }
        Err(payload) => {
            set_last_error(panic_message(&*payload));
            ErrorCode::Generic as i32
        }
    }
}

fn panic_message(payload: &dyn Any) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        format!("panic in FFI: {s}")
    } else if let Some(s) = payload.downcast_ref::<String>() {
        format!("panic in FFI: {s}")
    } else {
        "panic in FFI (non-string payload)".to_string()
    }
}

/// Map a `SpanzipError` to its FFI error code.
pub(crate) fn error_code_for(err: &SpanzipError) -> ErrorCode {
    match err {
        SpanzipError::Io(io) => match io.kind() {
            std::io::ErrorKind::NotFound => ErrorCode::FileNotFound,
            std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            // Stable since 1.83; older toolchains hit the wildcard.
            _ => ErrorCode::Io,
        },
        SpanzipError::InvalidArgument(_) => ErrorCode::InvalidArgument,
        SpanzipError::UnsupportedFormat(_) => ErrorCode::UnsupportedFormat,
        SpanzipError::Corrupted { .. } => ErrorCode::CorruptedArchive,
        SpanzipError::WrongPassword => ErrorCode::WrongPassword,
        SpanzipError::MissingVolume { .. } => ErrorCode::MissingVolume,
        SpanzipError::Canceled => ErrorCode::OperationCanceled,
        SpanzipError::FeatureDisabled(_) => ErrorCode::FeatureDisabled,
        SpanzipError::EntryNotFound(_) => ErrorCode::EntryNotFound,
        SpanzipError::PathTraversalBlocked(_) => ErrorCode::PathTraversal,
        SpanzipError::ZipBombSuspected { .. } => ErrorCode::ZipBomb,
        SpanzipError::BackendError(_) => ErrorCode::BackendError,
    }
}

/// Read a `(ptr, len)` pair as UTF-8. `len == 0` is a valid empty string
/// only when `ptr` is non-null; a null pointer is always rejected.
///
/// # Safety
/// Caller must ensure `ptr` points to at least `len` bytes of readable
/// memory and that the bytes outlive the returned slice.
pub(crate) unsafe fn read_utf8<'a>(
    ptr: *const c_char,
    len: usize,
) -> Result<&'a str, SpanzipError> {
    if ptr.is_null() {
        return Err(SpanzipError::InvalidArgument("null string pointer"));
    }
    // SAFETY: caller contract — `ptr` references `len` valid bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
    std::str::from_utf8(bytes)
        .map_err(|_| SpanzipError::InvalidArgument("string is not valid UTF-8"))
}

/// Variant that accepts NULL → `None` (for optional strings like password).
///
/// # Safety
/// Same contract as [`read_utf8`] when `ptr` is non-null.
pub(crate) unsafe fn read_optional_utf8<'a>(
    ptr: *const c_char,
    len: usize,
) -> Result<Option<&'a str>, SpanzipError> {
    if ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: forwarded.
    unsafe { read_utf8(ptr, len).map(Some) }
}
