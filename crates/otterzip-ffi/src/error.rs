//! FFI error codes + TLS message storage.
//! See `ffi-api.md` §2.

use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;

#[repr(i32)]
#[derive(Copy, Clone, Debug)]
pub enum ErrorCode {
    Ok                 = 0,
    Generic            = -1,
    InvalidArgument    = -2,
    InvalidHandle      = -3,
    OutOfMemory        = -4,

    Io                 = -10,
    FileNotFound       = -11,
    PermissionDenied   = -12,
    DiskFull           = -13,

    UnsupportedFormat  = -20,
    CorruptedArchive   = -21,
    WrongPassword      = -22,
    MissingVolume      = -23,
    EntryNotFound      = -24,
    IteratorEnd        = -25,

    OperationCanceled  = -30,
    FeatureDisabled    = -40,
    PathTraversal      = -41,
    ZipBomb            = -42,
    BackendError       = -50,
}

/// Convenience for the success path inside [`crate::util::catch_unwind_to_error`].
pub(crate) const OK: i32 = ErrorCode::Ok as i32;

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

pub(crate) fn set_last_error(msg: impl Into<Vec<u8>>) {
    let c = CString::new(msg).unwrap_or_else(|_| CString::new("invalid error message").unwrap());
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(c));
}

pub(crate) fn clear_last_error() {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = None);
}

#[no_mangle]
pub extern "C" fn otterzip_last_error_message() -> *const c_char {
    LAST_ERROR.with(|slot| match &*slot.borrow() {
        Some(c) => c.as_ptr(),
        None => ptr::null(),
    })
}

#[no_mangle]
pub extern "C" fn otterzip_clear_last_error() {
    clear_last_error();
}
