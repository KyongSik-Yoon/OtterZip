//! `x` (full paths) and `e` (flatten) — extract one or more archives.

use crate::cli::ExtractArgs;
use crate::commands::open_archive;
use crate::{exit, progress};
use anyhow::Result;
use otterzip_core::{ExtractOptions, OverwritePolicy};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

pub fn run(args: &ExtractArgs, flatten: bool, quiet: bool) -> Result<i32> {
    let mut worst = exit::EXIT_OK;
    for archive in &args.archives {
        if let Err(e) = extract_one(archive, args, flatten, quiet) {
            eprintln!("error: {}: {e:#}", archive.display());
            worst = worst.max(exit::code_for_anyhow(&e));
        }
    }
    Ok(worst)
}

fn extract_one(path: &Path, args: &ExtractArgs, flatten: bool, quiet: bool) -> Result<()> {
    let dest: PathBuf = args
        .output
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let overwrite = if args.overwrite_all {
        OverwritePolicy::Always
    } else if args.skip_existing {
        // Core treats IfNewer / AskCallback as "skip existing" in a
        // non-interactive context.
        OverwritePolicy::IfNewer
    } else if quiet {
        OverwritePolicy::Always
    } else {
        OverwritePolicy::Never
    };

    let opts = ExtractOptions {
        destination: dest,
        overwrite,
        flatten_paths: flatten,
        password: args.password.clone().map(Zeroizing::new),
        ..Default::default()
    };

    // `_merged` keeps the concatenated temp file alive (raw split inputs)
    // until extraction finishes; it deletes itself at end of scope.
    let (archive, _merged) = open_archive(path, args.password.as_deref())?;
    let mut sink = progress::make_sink(quiet);
    let report = archive.extract_all(&opts, Some(&mut sink))?;
    progress::finish(quiet);
    if !quiet {
        eprintln!(
            "Extracted {} entries ({} bytes) from {}",
            report.entries_extracted,
            report.bytes_written,
            path.display()
        );
    }
    Ok(())
}
