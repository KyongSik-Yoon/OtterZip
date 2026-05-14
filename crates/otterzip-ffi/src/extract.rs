//! Extraction FFI. See `ffi-api.md` §8.

use std::ffi::c_void;
use std::os::raw::c_char;
use std::path::PathBuf;

use otterzip_core::{
    Archive, ExtractOptions, OverwritePolicy, Progress, ProgressSink, OtterzipError,
};

use crate::archive::{archive_ref, OtterzipArchive};
use crate::error::{ErrorCode, OK};
use crate::util::{catch_unwind_to_error, read_optional_utf8, read_utf8};

/// Mirror of the C `OtterzipExtractOptions` struct. Field order and types
/// **must** match `ffi-api.md` §8.1; cbindgen reproduces this struct in the
/// generated header so any change here is an ABI change.
#[repr(C)]
pub struct OtterzipExtractOptions {
    pub destination_utf8: *const c_char,
    pub destination_len: usize,
    pub overwrite_policy: u32,
    pub flatten_paths: u8,
    pub preserve_permissions: u8,
    pub preserve_timestamps: u8,
    pub follow_symlinks: u8,
    pub block_path_traversal: u8,
    /// PR-7A (Phase 7): propagate `Zone.Identifier` ADS from the source
    /// archive to extracted files so SmartScreen / UAC keep behaving
    /// correctly. ABI-compatible reuse of one of the original 3 reserved
    /// padding bytes — older callers initialising the struct to zero get
    /// the safe "off" default automatically.
    pub preserve_zone_identifier: u8,
    pub _reserved: [u8; 2],
    /// ZIP-bomb defence threshold. `0` disables the check; otherwise an
    /// entry whose uncompressed/compressed ratio strictly exceeds this
    /// limit aborts extraction. Default in [`otterzip_core::ExtractOptions`]
    /// is 1000 — callers should pass that explicitly to keep the FFI
    /// layer free of hidden defaults.
    pub max_compression_ratio: u32,
    /// Phase 7 — cumulative ratio gate. `0` disables.
    pub max_total_compression_ratio: u32,
    /// Phase 7 — absolute output byte cap. `0` disables.
    pub max_total_output_bytes: u64,
    pub password_utf8: *const c_char,
    pub password_len: usize,
    pub entry_filter_utf8: *const c_char,
    pub entry_filter_len: usize,
}

/// Mirror of `OtterzipProgressView` from `ffi-api.md` §8.2. Passed by const
/// pointer to the C callback.
///
/// ABI v9 (compress sprint follow-up) appends
/// `current_entry_bytes_processed` + `current_entry_bytes_total`
/// — trailing-append, so v8 consumers that only read the leading
/// fields stay binary-compatible. New fields surface the
/// per-file streaming progress so the host can render a second
/// "current file" progress bar separate from the archive-wide one.
/// Older consumers ignoring the trailing fields see no behavioural
/// change.
#[repr(C)]
pub struct OtterzipProgressView {
    pub bytes_processed: u64,
    pub bytes_total: u64,
    pub entries_processed: u32,
    pub entries_total: u32,
    pub current_entry_utf8: *const c_char,
    pub current_entry_len: usize,
    pub phase: u32,
    pub elapsed_ms: u64,
    /// ABI v9 — bytes processed within the currently in-flight entry
    /// (large-file streaming path only; 0 elsewhere).
    pub current_entry_bytes_processed: u64,
    /// ABI v9 — total bytes the in-flight entry will occupy when
    /// finished (large-file streaming path only; 0 elsewhere).
    pub current_entry_bytes_total: u64,
}

/// Mirror of `OtterzipExtractReport`.
#[repr(C)]
#[derive(Default)]
pub struct OtterzipExtractReport {
    pub entries_extracted: u32,
    pub entries_skipped: u32,
    pub bytes_written: u64,
    pub warnings_count: u32,
    pub elapsed_ms: u64,
}

/// Mirror of `OtterzipTestReport` (`ffi-api.md` §10).
#[repr(C)]
#[derive(Default)]
pub struct OtterzipTestReport {
    pub entries_tested: u32,
    pub entries_corrupted: u32,
    pub elapsed_ms: u64,
}

/// C function pointer type for progress reporting. Returns 0 to continue,
/// non-zero (typically negative) to request cancellation.
pub type OtterzipProgressCb =
    Option<extern "C" fn(progress: *const OtterzipProgressView, user_data: *mut c_void) -> i32>;

fn overwrite_from_u32(value: u32) -> Result<OverwritePolicy, OtterzipError> {
    match value {
        0 => Ok(OverwritePolicy::Never),
        1 => Ok(OverwritePolicy::Always),
        2 => Ok(OverwritePolicy::IfNewer),
        3 => Ok(OverwritePolicy::AskCallback),
        _ => Err(OtterzipError::InvalidArgument("overwrite policy out of range")),
    }
}

/// Adapter that turns a C callback into a [`ProgressSink`]. Holds the raw
/// C function pointer + opaque user pointer.
///
/// The Sprint 5 parallel extractor sends `&mut dyn ProgressSink` across
/// rayon worker threads via a `Mutex`, which forces `ProgressSink: Send`.
/// We assert `Send` for `CallbackSink` manually because the contained
/// `*mut c_void` is opaque; the C ABI contract (`ffi-api.md` §0.1 #7)
/// already requires the caller to handle thread-safety on the user
/// pointer side, so it's safe to ferry across threads at the Rust layer.
pub(crate) struct CallbackSink {
    pub(crate) cb: extern "C" fn(*const OtterzipProgressView, *mut c_void) -> i32,
    pub(crate) user: *mut c_void,
    /// Reusable scratch for entry-name CString → keeps the pointer stable
    /// for the duration of one callback invocation.
    pub(crate) name_buf: Vec<u8>,
}

impl CallbackSink {
    /// Build the adapter directly from a non-NULL C function pointer.
    /// Used by [`crate::create::otterzip_archive_add_directory_p`] which
    /// shares the same on-wire protocol but only invokes the callback
    /// during compress.
    pub(crate) fn new(
        cb: extern "C" fn(*const OtterzipProgressView, *mut c_void) -> i32,
        user: *mut c_void,
    ) -> Self {
        Self { cb, user, name_buf: Vec::new() }
    }
}

// SAFETY: see doc comment on CallbackSink — the C ABI contract pushes
// thread-safety responsibility onto the caller for `user_data`, and the
// function pointer itself is intrinsically Send.
unsafe impl Send for CallbackSink {}

impl ProgressSink for CallbackSink {
    fn update(&mut self, p: &Progress) -> bool {
        // Build a NUL-terminated copy of `current_entry` once per tick.
        // We ignore interior NUL bytes (treat as truncated) — realistic
        // archive paths never contain them.
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

        // Per `ffi-api.md` §8.2: 0 = continue, anything else = cancel.
        let rc = (self.cb)(&view, self.user);
        rc == 0
    }
}

/// Extract every entry under `opts.destination`. Returns
/// [`ErrorCode::OperationCanceled`] if the progress callback signals cancel.
///
/// `out_report` may be NULL — when non-null we always write a report on the
/// success path, and a zeroed report on the cancel path.
#[no_mangle]
pub extern "C" fn otterzip_archive_extract_all(
    handle: *mut OtterzipArchive,
    opts: *const OtterzipExtractOptions,
    progress_cb: OtterzipProgressCb,
    user_data: *mut c_void,
    out_report: *mut OtterzipExtractReport,
) -> i32 {
    catch_unwind_to_error(|| {
        tracing::debug!(target: "otterzip::ffi", "otterzip_archive_extract_all entered");
        if opts.is_null() {
            return Err(OtterzipError::InvalidArgument("opts is null"));
        }
        // SAFETY: caller contract — handle came from `otterzip_archive_open`.
        let archive = match unsafe { archive_ref(handle.cast_const()) } {
            Some(a) => a,
            None => return Ok(ErrorCode::InvalidHandle as i32),
        };

        // SAFETY: opts checked non-null above.
        let opts_c = unsafe { &*opts };
        // SAFETY: caller-provided UTF-8 slices (length-prefixed).
        let dest = unsafe { read_utf8(opts_c.destination_utf8, opts_c.destination_len)? };
        let password =
            unsafe { read_optional_utf8(opts_c.password_utf8, opts_c.password_len)? };
        let filter_str =
            unsafe { read_optional_utf8(opts_c.entry_filter_utf8, opts_c.entry_filter_len)? };
        tracing::info!(
            target: "otterzip::ffi",
            dest = %dest,
            has_password = password.is_some(),
            has_filter = filter_str.is_some(),
            "FFI extract_all params parsed"
        );

        let core_opts = ExtractOptions {
            destination: PathBuf::from(dest),
            overwrite: overwrite_from_u32(opts_c.overwrite_policy)?,
            flatten_paths: opts_c.flatten_paths != 0,
            preserve_permissions: opts_c.preserve_permissions != 0,
            preserve_timestamps: opts_c.preserve_timestamps != 0,
            follow_symlinks: opts_c.follow_symlinks != 0,
            block_path_traversal: opts_c.block_path_traversal != 0,
            max_compression_ratio: opts_c.max_compression_ratio,
            max_total_compression_ratio: opts_c.max_total_compression_ratio,
            max_total_output_bytes: opts_c.max_total_output_bytes,
            entry_filter: filter_str.map(|s| {
                s.split('\n')
                    .filter(|p| !p.is_empty())
                    .map(str::to_owned)
                    .collect()
            }),
            password: password.map(|p| zeroize::Zeroizing::new(p.to_owned())),
            preserve_zone_identifier: opts_c.preserve_zone_identifier != 0,
        };

        let mut report = OtterzipExtractReport::default();

        let result = match progress_cb {
            Some(cb) => {
                let mut sink = CallbackSink {
                    cb,
                    user: user_data,
                    name_buf: Vec::with_capacity(64),
                };
                run_extract(archive, &core_opts, Some(&mut sink))
            }
            None => run_extract::<CallbackSink>(archive, &core_opts, None),
        };

        match result {
            Ok(core_report) => {
                report.entries_extracted = core_report.entries_extracted;
                report.entries_skipped = core_report.entries_skipped;
                report.bytes_written = core_report.bytes_written;
                report.warnings_count =
                    u32::try_from(core_report.warnings.len()).unwrap_or(u32::MAX);
                report.elapsed_ms =
                    u64::try_from(core_report.elapsed.as_millis()).unwrap_or(u64::MAX);
                if !out_report.is_null() {
                    // SAFETY: null-checked.
                    unsafe { *out_report = report };
                }
                Ok(OK)
            }
            Err(err) => Err(err),
        }
    })
}

fn run_extract<S: ProgressSink>(
    archive: &Archive,
    opts: &ExtractOptions,
    sink: Option<&mut S>,
) -> otterzip_core::Result<otterzip_core::ExtractReport> {
    archive.extract_all(opts, sink)
}
