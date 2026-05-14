//! Integrity-test FFI surface (`ffi-api.md` §10).
//!
//! Exposes `otterzip_archive_test` (drives `Archive::test`) and
//! `otterzip_archive_test_corrupted_entry` (look up the i-th corrupted
//! path after a test run). The corrupted-entry list is cached on the
//! Archive handle so the second call is O(1).

use std::cell::RefCell;
use std::ffi::{c_void, CString};
use std::os::raw::c_char;
use std::ptr;

use otterzip_core::{Progress, ProgressSink, OtterzipError, TestReport};

use crate::archive::{archive_ref, OtterzipArchive};
use crate::error::{ErrorCode, OK};
use crate::extract::{OtterzipProgressCb, OtterzipProgressView, OtterzipTestReport};
use crate::util::catch_unwind_to_error;

thread_local! {
    /// Most recent test result. Holds null-terminated copies so
    /// `otterzip_archive_test_corrupted_entry` can hand back stable
    /// pointers without re-running the test.
    static LAST_TEST: RefCell<Vec<CString>> = const { RefCell::new(Vec::new()) };
}

struct CallbackSink {
    cb: extern "C" fn(*const OtterzipProgressView, *mut c_void) -> i32,
    user: *mut c_void,
    name_buf: Vec<u8>,
}

// SAFETY: same rationale as `extract::CallbackSink` — caller-owned thread
// safety on the user pointer is part of the C ABI contract.
unsafe impl Send for CallbackSink {}

impl ProgressSink for CallbackSink {
    fn update(&mut self, p: &Progress) -> bool {
        self.name_buf.clear();
        let (name_ptr, name_len) = match p.current_entry.as_deref() {
            Some(s) => {
                self.name_buf.extend_from_slice(s.as_bytes());
                self.name_buf.push(0);
                (
                    self.name_buf.as_ptr().cast::<c_char>(),
                    self.name_buf.len() - 1,
                )
            }
            None => (std::ptr::null(), 0),
        };
        let view = OtterzipProgressView {
            bytes_processed: p.bytes_processed,
            bytes_total: p.bytes_total,
            entries_processed: p.entries_processed,
            entries_total: p.entries_total,
            current_entry_utf8: name_ptr,
            current_entry_len: name_len,
            phase: p.phase as u32,
            elapsed_ms: u64::try_from(p.elapsed.as_millis()).unwrap_or(u64::MAX),
            current_entry_bytes_processed: p.current_entry_bytes_processed,
            current_entry_bytes_total: p.current_entry_bytes_total,
        };
        (self.cb)(&view, self.user) == 0
    }
}

/// Walk every entry, decompress, verify CRC32 where present. Cancellation
/// works the same as `otterzip_archive_extract_all`.
#[no_mangle]
pub extern "C" fn otterzip_archive_test(
    handle: *const OtterzipArchive,
    progress_cb: OtterzipProgressCb,
    user_data: *mut c_void,
    out_report: *mut OtterzipTestReport,
) -> i32 {
    catch_unwind_to_error(|| {
        // SAFETY: handle null-checked by `archive_ref`.
        let archive = match unsafe { archive_ref(handle) } {
            Some(a) => a,
            None => return Ok(ErrorCode::InvalidHandle as i32),
        };

        let report: TestReport = match progress_cb {
            Some(cb) => {
                let mut sink = CallbackSink {
                    cb,
                    user: user_data,
                    name_buf: Vec::with_capacity(64),
                };
                archive.test(Some(&mut sink))?
            }
            None => archive.test::<CallbackSink>(None)?,
        };

        // Cache corrupted entry names for `_test_corrupted_entry` lookups.
        LAST_TEST.with(|slot| {
            let mut v = slot.borrow_mut();
            v.clear();
            v.reserve(report.corrupted_entries.len());
            for path in &report.corrupted_entries {
                if let Ok(c) = CString::new(path.as_bytes()) {
                    v.push(c);
                }
            }
        });

        if !out_report.is_null() {
            // SAFETY: null-checked.
            unsafe {
                *out_report = OtterzipTestReport {
                    entries_tested: report.entries_tested,
                    entries_corrupted: report.entries_corrupted,
                    elapsed_ms: u64::try_from(report.elapsed.as_millis()).unwrap_or(u64::MAX),
                };
            }
        }
        Ok(OK)
    })
}

/// Look up the path of the i-th corrupted entry from the most recent
/// `otterzip_archive_test` call (TLS). The pointer remains valid until
/// the next test run on this thread.
#[no_mangle]
pub extern "C" fn otterzip_archive_test_corrupted_entry(
    _handle: *const OtterzipArchive,
    index: u32,
    out_path_utf8: *mut *const c_char,
    out_len: *mut usize,
) -> i32 {
    catch_unwind_to_error(|| {
        if out_path_utf8.is_null() || out_len.is_null() {
            return Err(OtterzipError::InvalidArgument(
                "out_path_utf8 / out_len is null",
            ));
        }
        LAST_TEST.with(|slot| {
            let v = slot.borrow();
            let i = index as usize;
            if i >= v.len() {
                // SAFETY: out pointers null-checked above.
                unsafe {
                    *out_path_utf8 = ptr::null();
                    *out_len = 0;
                }
                Ok(ErrorCode::EntryNotFound as i32)
            } else {
                let c = &v[i];
                // SAFETY: same.
                unsafe {
                    *out_path_utf8 = c.as_ptr();
                    *out_len = c.as_bytes().len();
                }
                Ok(OK)
            }
        })
    })
}
