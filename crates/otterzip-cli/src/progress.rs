//! Terminal progress rendering — a stderr `ProgressSink` closure.
//!
//! `otterzip-core`'s `ProgressSink` has a blanket impl for
//! `FnMut(&Progress) -> bool + Send`, so a closure is a sink. We render
//! to **stderr** (keeps `l` stdout pipe-clean) and only when stderr is
//! an interactive terminal and the caller didn't request quiet.

use otterzip_core::Progress;
use std::io::{IsTerminal, Write};

/// Build a progress sink. When `quiet` or stderr is not a TTY the
/// returned closure is a no-op (still returns `true` to keep the
/// operation running — `false` would request cancellation).
pub fn make_sink(quiet: bool) -> impl FnMut(&Progress) -> bool + Send {
    let show = !quiet && std::io::stderr().is_terminal();
    let mut last_pct: i32 = -1;
    move |p: &Progress| {
        if show && p.bytes_total > 0 {
            let pct = ((p.bytes_processed.min(p.bytes_total) * 100) / p.bytes_total) as i32;
            if pct != last_pct {
                last_pct = pct;
                let name = p.current_entry.as_deref().unwrap_or("");
                // Truncate the name so the carriage-return line stays
                // a single terminal row.
                let shown: String = name.chars().take(50).collect();
                eprint!("\r{pct:3}%  {shown:<50}");
                let _ = std::io::stderr().flush();
            }
        }
        true
    }
}

/// Clear the progress line after an operation completes.
pub fn finish(quiet: bool) {
    if !quiet && std::io::stderr().is_terminal() {
        eprintln!();
    }
}
