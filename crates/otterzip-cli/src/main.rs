//! OtterZip command-line tool (`otterzip`).
//!
//! A thin layer over `otterzip-core` — no archive logic lives here. See
//! `docs/03-api/cli.md` for the verb/switch reference and exit codes.

mod cli;
mod commands;
mod exit;
mod format_map;
mod output;
mod progress;

use clap::Parser;
use cli::{Cli, Command};
use std::process::ExitCode;

fn main() -> ExitCode {
    // Korean (and any non-ASCII) filenames need a UTF-8 console on
    // Windows; restored on drop. No-op elsewhere.
    let _cp = console::enable_utf8();

    let cli = Cli::parse();
    let quiet = cli.assume_yes;

    let result = match &cli.command {
        Command::Add(a) => commands::add::run(a, quiet),
        Command::Extract(x) => commands::extract::run(x, false, quiet),
        Command::ExtractFlat(e) => commands::extract::run(e, true, quiet),
        Command::Test(t) => commands::test::run(t, quiet),
        Command::List(l) => commands::list::run(l),
        Command::BatchCreate(a) => commands::batch::create(a, quiet),
        Command::BatchExtract(x) => commands::batch::extract(x, quiet),
        Command::BatchTest(t) => commands::batch::test(t, quiet),
    };

    match result {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(2)),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(u8::try_from(exit::code_for_anyhow(&e)).unwrap_or(2))
        }
    }
}

/// Windows console code-page handling. On non-Windows this is an empty
/// guard so call sites stay platform-agnostic.
#[cfg(windows)]
mod console {
    // Raw FFI avoids pulling `windows-sys` for three trivial calls.
    extern "system" {
        fn GetConsoleOutputCP() -> u32;
        fn SetConsoleOutputCP(cp: u32) -> i32;
        fn SetConsoleCP(cp: u32) -> i32;
    }
    const CP_UTF8: u32 = 65001;

    pub struct CpGuard(u32);

    /// Switch the console to UTF-8 for the process lifetime, returning a
    /// guard that restores the previous output code page on drop.
    pub fn enable_utf8() -> CpGuard {
        // SAFETY: the console CP APIs are always callable; they no-op
        // when there's no attached console (e.g. piped), which is
        // harmless.
        unsafe {
            let prev = GetConsoleOutputCP();
            SetConsoleOutputCP(CP_UTF8);
            SetConsoleCP(CP_UTF8);
            CpGuard(prev)
        }
    }

    impl Drop for CpGuard {
        fn drop(&mut self) {
            // SAFETY: restoring the previously-read code page.
            unsafe {
                SetConsoleOutputCP(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
mod console {
    pub struct CpGuard;
    pub fn enable_utf8() -> CpGuard {
        CpGuard
    }
}
