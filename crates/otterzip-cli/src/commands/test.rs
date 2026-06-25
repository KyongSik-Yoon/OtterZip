//! `t` — test archive integrity. Exit 5 when any entry is corrupted.

use crate::cli::TestArgs;
use crate::commands::open_archive;
use crate::{exit, progress};
use anyhow::Result;
use std::path::Path;

pub fn run(args: &TestArgs, quiet: bool) -> Result<i32> {
    let mut worst = exit::EXIT_OK;
    for archive in &args.archives {
        match test_one(archive, args.password.as_deref(), quiet) {
            Ok(code) => worst = worst.max(code),
            Err(e) => {
                eprintln!("error: {}: {e:#}", archive.display());
                worst = worst.max(exit::code_for_anyhow(&e));
            }
        }
    }
    Ok(worst)
}

fn test_one(path: &Path, password: Option<&str>, quiet: bool) -> Result<i32> {
    let (archive, _merged) = open_archive(path, password)?;
    let mut sink = progress::make_sink(quiet);
    let report = archive.test(Some(&mut sink))?;
    progress::finish(quiet);

    if report.entries_corrupted > 0 {
        eprintln!(
            "{}: {} of {} entries CORRUPTED",
            path.display(),
            report.entries_corrupted,
            report.entries_tested
        );
        for c in &report.corrupted_entries {
            eprintln!("  {c}");
        }
        return Ok(5);
    }
    if !quiet {
        println!("{}: OK ({} entries)", path.display(), report.entries_tested);
    }
    Ok(exit::EXIT_OK)
}
