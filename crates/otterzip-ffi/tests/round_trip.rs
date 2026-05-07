//! FFI round-trip integration test (Sprint 2 walking skeleton).
//!
//! Exercises the C ABI surface from Rust — opens a fixture ZIP through the
//! exported `otterzip_archive_open`, iterates entries via the C iterator,
//! extracts, and verifies the on-disk result. This is the same path a C#
//! P/Invoke caller would take.

use std::ffi::c_void;
use std::fs;
use std::io::Write;
use std::ptr;

use otterzip_ffi::{
    otterzip_archive_add_directory, otterzip_archive_close, otterzip_archive_commit,
    otterzip_archive_create, otterzip_archive_extract_all, otterzip_archive_format,
    otterzip_archive_open, otterzip_detect_format, otterzip_iterator_free, otterzip_iterator_new,
    otterzip_iterator_next, OtterzipArchive, OtterzipCreateOptions, OtterzipEntryView,
    OtterzipExtractOptions, OtterzipExtractReport, OtterzipIterator, OtterzipProgressView,
};

const OTTERZIP_OK: i32 = 0;
const OTTERZIP_ERR_ITERATOR_END: i32 = -25;
const FORMAT_ZIP: u32 = 1;

fn build_fixture_zip(out: &std::path::Path) {
    let file = fs::File::create(out).expect("create fixture");
    let mut writer = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    writer.start_file("readme.txt", opts).unwrap();
    writer.write_all(b"hello via ffi\n").unwrap();

    writer.add_directory("nested/", opts).unwrap();
    writer.start_file("nested/data.bin", opts).unwrap();
    writer.write_all(&[0u8, 1, 2, 3, 4, 5, 6, 7]).unwrap();

    writer.finish().unwrap();
}

fn path_arg(p: &std::path::Path) -> (Vec<u8>, usize) {
    let s = p.to_str().expect("utf8 path");
    let bytes = s.as_bytes().to_vec();
    let len = bytes.len();
    (bytes, len)
}

#[test]
fn detect_format_via_ffi() {
    let td = tempfile::tempdir().unwrap();
    let zip_path = td.path().join("fixture.zip");
    build_fixture_zip(&zip_path);

    let (buf, len) = path_arg(&zip_path);
    let mut fmt: u32 = 0;
    let rc = otterzip_detect_format(buf.as_ptr().cast(), len, &mut fmt);
    assert_eq!(rc, OTTERZIP_OK);
    assert_eq!(fmt, FORMAT_ZIP);
}

#[test]
fn open_iterate_close_via_ffi() {
    let td = tempfile::tempdir().unwrap();
    let zip_path = td.path().join("fixture.zip");
    build_fixture_zip(&zip_path);

    let (buf, len) = path_arg(&zip_path);
    let mut handle: *mut OtterzipArchive = ptr::null_mut();
    let rc = otterzip_archive_open(
        buf.as_ptr().cast(),
        len,
        0, // OpenMode::Read
        ptr::null(),
        0,
        &mut handle,
    );
    assert_eq!(rc, OTTERZIP_OK);
    assert!(!handle.is_null());

    let mut fmt: u32 = 0;
    assert_eq!(otterzip_archive_format(handle, &mut fmt), OTTERZIP_OK);
    assert_eq!(fmt, FORMAT_ZIP);

    let mut iter: *mut OtterzipIterator = ptr::null_mut();
    assert_eq!(otterzip_iterator_new(handle, &mut iter), OTTERZIP_OK);
    assert!(!iter.is_null());

    let mut names = Vec::new();
    let mut view = OtterzipEntryView::default();
    loop {
        let rc = otterzip_iterator_next(iter, &mut view);
        if rc == OTTERZIP_ERR_ITERATOR_END {
            break;
        }
        assert_eq!(rc, OTTERZIP_OK);
        // SAFETY: per ABI contract, path pointer is valid until next call.
        let path = unsafe {
            let bytes = std::slice::from_raw_parts(view.path_utf8.cast::<u8>(), view.path_len);
            std::str::from_utf8(bytes).unwrap().to_owned()
        };
        names.push(path);
    }
    assert!(names.iter().any(|n| n == "readme.txt"));
    assert!(names.iter().any(|n| n == "nested/data.bin"));
    assert!(names.iter().any(|n| n == "nested/"));

    otterzip_iterator_free(iter);
    otterzip_archive_close(handle);
}

extern "C" fn count_progress_ticks(
    _view: *const OtterzipProgressView,
    user: *mut c_void,
) -> i32 {
    if !user.is_null() {
        // SAFETY: caller passes &mut u32; we increment in-place.
        unsafe {
            *user.cast::<u32>() += 1;
        }
    }
    0
}

#[test]
fn extract_all_via_ffi_with_progress_callback() {
    let td = tempfile::tempdir().unwrap();
    let zip_path = td.path().join("fixture.zip");
    let out_dir = td.path().join("out");
    build_fixture_zip(&zip_path);

    let (path_buf, path_len) = path_arg(&zip_path);
    let mut handle: *mut OtterzipArchive = ptr::null_mut();
    let rc = otterzip_archive_open(
        path_buf.as_ptr().cast(),
        path_len,
        0,
        ptr::null(),
        0,
        &mut handle,
    );
    assert_eq!(rc, OTTERZIP_OK);

    let dest = out_dir.to_str().unwrap().as_bytes().to_vec();
    let opts = OtterzipExtractOptions {
        destination_utf8: dest.as_ptr().cast(),
        destination_len: dest.len(),
        overwrite_policy: 1, // Always
        flatten_paths: 0,
        preserve_permissions: 1,
        preserve_timestamps: 1,
        follow_symlinks: 0,
        block_path_traversal: 1,
        preserve_zone_identifier: 0, // PR-7A: off for round-trip test
        _reserved: [0; 2],
        max_compression_ratio: 1000,
        max_total_compression_ratio: 100,
        max_total_output_bytes: 16 * 1024 * 1024 * 1024,
        password_utf8: ptr::null(),
        password_len: 0,
        entry_filter_utf8: ptr::null(),
        entry_filter_len: 0,
    };

    let mut tick_count: u32 = 0;
    let mut report = OtterzipExtractReport::default();
    let rc = otterzip_archive_extract_all(
        handle,
        &opts,
        Some(count_progress_ticks),
        std::ptr::addr_of_mut!(tick_count).cast::<c_void>(),
        &mut report,
    );
    assert_eq!(rc, OTTERZIP_OK);
    assert!(report.entries_extracted >= 2);
    assert!(report.bytes_written > 0);
    assert!(tick_count > 0, "progress callback was never invoked");

    otterzip_archive_close(handle);

    // Verify on-disk content.
    let readme = fs::read(out_dir.join("readme.txt")).unwrap();
    assert_eq!(readme, b"hello via ffi\n");
    let data = fs::read(out_dir.join("nested").join("data.bin")).unwrap();
    assert_eq!(data, [0u8, 1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn create_then_extract_via_ffi() {
    let td = tempfile::tempdir().unwrap();
    let src = td.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("hello.txt"), b"hi from ffi create\n").unwrap();

    let archive_path = td.path().join("created.zip");
    let (path_buf, path_len) = path_arg(&archive_path);

    let opts = OtterzipCreateOptions {
        format: 1,        // Zip
        compression: 1,   // Deflate
        compression_level: 5,
        solid: 0,
        preserve_permissions: 1,
        preserve_timestamps: 1,
        encryption: 0,
        password_utf8: ptr::null(),
        password_len: 0,
        volume_size_bytes: 0,
        thread_count: 0,
        exclude_system_metadata: 0, // ABI v6 trailing field — off for round-trip
    };

    let mut handle: *mut OtterzipArchive = ptr::null_mut();
    let rc = otterzip_archive_create(
        path_buf.as_ptr().cast(),
        path_len,
        &opts,
        &mut handle,
    );
    assert_eq!(rc, OTTERZIP_OK);
    assert!(!handle.is_null());

    // add_directory and commit (commit consumes the handle).
    let (src_buf, src_len) = path_arg(&src);
    let rc = otterzip_archive_add_directory(
        handle,
        src_buf.as_ptr().cast(),
        src_len,
        ptr::null(),
        0,
    );
    assert_eq!(rc, OTTERZIP_OK);

    let rc = otterzip_archive_commit(handle);
    assert_eq!(rc, OTTERZIP_OK);

    // Now re-open via FFI and verify content.
    assert!(archive_path.exists());
    let (re_path, re_len) = path_arg(&archive_path);
    let mut read_handle: *mut OtterzipArchive = ptr::null_mut();
    let rc = otterzip_archive_open(
        re_path.as_ptr().cast(),
        re_len,
        0,
        ptr::null(),
        0,
        &mut read_handle,
    );
    assert_eq!(rc, OTTERZIP_OK);
    let mut fmt = 0u32;
    assert_eq!(otterzip_archive_format(read_handle, &mut fmt), OTTERZIP_OK);
    assert_eq!(fmt, FORMAT_ZIP);
    otterzip_archive_close(read_handle);
}

#[test]
fn open_invalid_path_returns_error() {
    let nonsense = b"D:/no-such-file-12345.zip";
    let mut handle: *mut OtterzipArchive = ptr::null_mut();
    let rc = otterzip_archive_open(
        nonsense.as_ptr().cast(),
        nonsense.len(),
        0,
        ptr::null(),
        0,
        &mut handle,
    );
    assert!(rc < 0, "expected error, got rc={rc}");
    assert!(handle.is_null());
}
