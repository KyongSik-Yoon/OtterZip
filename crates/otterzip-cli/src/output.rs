//! `l` (list) stdout rendering — columnar, pipe-friendly.
//!
//! Columns: right-aligned size, modified timestamp (UTC, `YYYY-MM-DD
//! HH:MM`), then the entry path verbatim (forward-slash, UTF-8). Dates
//! are computed from `SystemTime` with a civil-from-days conversion so
//! we avoid a `chrono`/`time` dependency.

use otterzip_core::Entry;
use std::time::{SystemTime, UNIX_EPOCH};

/// Print one archive's entries in a fixed-column layout, returning the
/// number of entries listed. Per-entry read errors are printed to
/// stderr by the caller; this function only formats successful entries.
pub fn print_entries(entries: &[Entry]) {
    println!("{:>12}  {:<16}  {}", "Size", "Modified (UTC)", "Name");
    println!("{:->12}  {:-<16}  {:-<40}", "", "", "");
    let mut total_size: u64 = 0;
    for e in entries {
        let size = if e.is_directory {
            0
        } else {
            e.uncompressed_size
        };
        total_size += size;
        let when = e.modified.map_or_else(|| "-".to_string(), format_mtime);
        let name = if e.is_directory {
            format!("{}/", e.path.trim_end_matches('/'))
        } else {
            e.path.clone()
        };
        println!("{size:>12}  {when:<16}  {name}");
    }
    println!("{:->12}  {:-<16}  {:-<40}", "", "", "");
    println!("{total_size:>12}  {:<16}  {} entries", "", entries.len());
}

/// `SystemTime` → `YYYY-MM-DD HH:MM` (UTC). Pre-epoch times render `-`.
fn format_mtime(t: SystemTime) -> String {
    let Ok(dur) = t.duration_since(UNIX_EPOCH) else {
        return "-".to_string();
    };
    let secs = dur.as_secs();
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (hh, mm) = ((tod / 3_600), (tod % 3_600) / 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02} {hh:02}:{mm:02}")
}

/// Howard Hinnant's days-from-epoch → civil (year, month, day).
/// `days` is days since 1970-01-01. Public-domain algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn epoch_is_1970() {
        assert_eq!(format_mtime(UNIX_EPOCH), "1970-01-01 00:00");
    }

    #[test]
    fn known_date() {
        // 2021-01-01 00:00:00 UTC = 1609459200
        let t = UNIX_EPOCH + Duration::from_secs(1_609_459_200);
        assert_eq!(format_mtime(t), "2021-01-01 00:00");
    }
}
