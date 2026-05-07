//! Entry iteration FFI. See `ffi-api.md` §6.

use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;

use otterzip_core::{Entry, EntryIter, OtterzipError};

use crate::archive::{archive_ref, OtterzipArchive};
use crate::error::{ErrorCode, OK};
use crate::util::catch_unwind_to_error;

/// POD view of an [`Entry`]. The string pointers inside this struct are
/// borrowed from the iterator's last-yielded entry — they remain valid only
/// until the next call to [`otterzip_iterator_next`] (or until the iterator is
/// freed). Callers must copy any data they wish to retain.
#[repr(C)]
#[derive(Default)]
pub struct OtterzipEntryView {
    pub path_utf8: *const c_char,
    pub path_len: usize,
    pub is_directory: u8,
    pub is_symlink: u8,
    pub is_encrypted: u8,
    pub _reserved: u8,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub compression: u32,
    pub encryption: u32,
    pub crc32: u32,
    pub modified_unix_ms: i64,
    pub accessed_unix_ms: i64,
    pub created_unix_ms: i64,
    pub attributes: u32,
    pub host_os: u32,
    pub comment_utf8: *const c_char,
    pub comment_len: usize,
}

/// Opaque iterator handle.
#[repr(C)]
pub struct OtterzipIterator {
    _private: [u8; 0],
}

/// Internal storage. We hold the boxed `EntryIter<'a>` together with the
/// `CString` buffers that back the most recently yielded entry's strings.
/// Borrowing a stable pointer out of an `Entry: String` directly is unsafe
/// because moving the Entry invalidates the slice — caching `CString`s here
/// keeps them pinned for the duration the caller observes the view.
struct IterState {
    /// Erased lifetime: see SAFETY in `otterzip_iterator_new`.
    iter: EntryIter<'static>,
    /// Last yielded entry's path, kept alive for the FFI view's pointer.
    last_path: Option<CString>,
    /// Last yielded entry's comment, if any.
    last_comment: Option<CString>,
}

fn to_unix_ms(t: Option<std::time::SystemTime>) -> i64 {
    t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(-1)
}

fn fill_view(view: &mut OtterzipEntryView, entry: &Entry, state: &mut IterState) {
    let path_c = CString::new(entry.path.as_bytes())
        .unwrap_or_else(|_| CString::new("invalid").unwrap());
    let path_len = path_c.as_bytes().len();
    state.last_path = Some(path_c);
    let path_ptr = state.last_path.as_ref().unwrap().as_ptr();

    let (comment_ptr, comment_len) = match entry.comment.as_deref() {
        Some(s) => {
            let c = CString::new(s).unwrap_or_else(|_| CString::new("").unwrap());
            let len = c.as_bytes().len();
            state.last_comment = Some(c);
            (state.last_comment.as_ref().unwrap().as_ptr(), len)
        }
        None => {
            state.last_comment = None;
            (ptr::null(), 0)
        }
    };

    view.path_utf8 = path_ptr;
    view.path_len = path_len;
    view.is_directory = u8::from(entry.is_directory);
    view.is_symlink = u8::from(entry.is_symlink);
    view.is_encrypted = u8::from(entry.encryption != otterzip_core::EncryptionMethod::None);
    view._reserved = 0;
    view.uncompressed_size = entry.uncompressed_size;
    view.compressed_size = entry.compressed_size;
    view.compression = entry.compression as u32;
    view.encryption = entry.encryption as u32;
    view.crc32 = entry.crc32.unwrap_or(0);
    view.modified_unix_ms = to_unix_ms(entry.modified);
    view.accessed_unix_ms = to_unix_ms(entry.accessed);
    view.created_unix_ms = to_unix_ms(entry.created);
    view.attributes = entry.attributes;
    view.host_os = entry.host_os as u32;
    view.comment_utf8 = comment_ptr;
    view.comment_len = comment_len;
}

/// Construct a new iterator over the entries of `handle`.
///
/// The iterator holds a borrow of the archive: while the iterator is alive
/// the caller must not close the underlying archive. Always free with
/// [`otterzip_iterator_free`] before [`crate::archive::otterzip_archive_close`].
#[no_mangle]
pub extern "C" fn otterzip_iterator_new(
    handle: *const OtterzipArchive,
    out_iter: *mut *mut OtterzipIterator,
) -> i32 {
    catch_unwind_to_error(|| {
        if out_iter.is_null() {
            return Err(OtterzipError::InvalidArgument("out_iter is null"));
        }
        // SAFETY: archive lifetime is enforced by the caller per the
        // iterator/archive ordering contract documented above.
        let archive = match unsafe { archive_ref(handle) } {
            Some(a) => a,
            None => return Ok(ErrorCode::InvalidHandle as i32),
        };
        let iter = archive.entries()?;

        // SAFETY: We erase the iterator's lifetime to `'static` so we can
        // box it. The caller contract requires `handle` to outlive the
        // iterator (single-threaded use, free-before-close), which keeps
        // the borrow valid in practice.
        let iter_static: EntryIter<'static> =
            unsafe { std::mem::transmute::<EntryIter<'_>, EntryIter<'static>>(iter) };

        let state = Box::new(IterState {
            iter: iter_static,
            last_path: None,
            last_comment: None,
        });
        // SAFETY: out_iter null-checked above.
        unsafe {
            *out_iter = Box::into_raw(state).cast::<OtterzipIterator>();
        }
        Ok(OK)
    })
}

/// Advance the iterator. On `OTTERZIP_OK` the `out_entry` view is populated;
/// the embedded string pointers are valid until the next call. On end of
/// iteration returns [`ErrorCode::IteratorEnd`].
#[no_mangle]
pub extern "C" fn otterzip_iterator_next(
    iter: *mut OtterzipIterator,
    out_entry: *mut OtterzipEntryView,
) -> i32 {
    catch_unwind_to_error(|| {
        if iter.is_null() {
            return Ok(ErrorCode::InvalidHandle as i32);
        }
        if out_entry.is_null() {
            return Err(OtterzipError::InvalidArgument("out_entry is null"));
        }
        // SAFETY: iter came from `otterzip_iterator_new`, single-threaded use.
        let state = unsafe { &mut *iter.cast::<IterState>() };

        let next = state.iter.next();
        match next {
            None => Ok(ErrorCode::IteratorEnd as i32),
            Some(Err(err)) => Err(err),
            Some(Ok(entry)) => {
                // SAFETY: out_entry null-checked above.
                let view = unsafe { &mut *out_entry };
                *view = OtterzipEntryView::default();
                fill_view(view, &entry, state);
                Ok(OK)
            }
        }
    })
}

/// Free the iterator. NULL is a no-op.
#[no_mangle]
pub extern "C" fn otterzip_iterator_free(iter: *mut OtterzipIterator) {
    if iter.is_null() {
        return;
    }
    // SAFETY: came from `otterzip_iterator_new`.
    unsafe {
        drop(Box::from_raw(iter.cast::<IterState>()));
    }
}
