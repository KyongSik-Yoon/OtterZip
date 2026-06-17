//! Batch verbs: `bc` (one archive per input), `bx` / `bt` (multi-target
//! extract / test — thin wrappers since `x`/`t` already loop). Each
//! aggregates the worst exit code.

use crate::cli::{AddArgs, ExtractArgs, TestArgs};
use crate::commands::add::{self, CreateSpec};
use crate::commands::{extract as extract_cmd, test as test_cmd};
use crate::exit;
use crate::format_map::primary_ext;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// `bc` — compress each input into its OWN archive (`<name>.<ext>`),
/// next to the input. Default format ZIP unless `--format`.
pub fn create(args: &AddArgs, quiet: bool) -> Result<i32> {
    // For bc the `archive` positional is unused (each input names its
    // own output) — but clap still requires it; treat it as the first
    // input so `bc foo bar` compresses both.
    let spec = CreateSpec {
        format: args.format.as_deref(),
        level: args.level,
        password: args.password.as_deref(),
        volume: args.volume.as_deref(),
        threads: args.threads,
        storeroot: args.storeroot,
    };
    let format = add::batch_format(&spec)?;
    let ext = primary_ext(format);

    let mut targets: Vec<PathBuf> = Vec::with_capacity(args.inputs.len() + 1);
    targets.push(args.archive.clone());
    targets.extend(args.inputs.iter().cloned());

    let mut worst = exit::EXIT_OK;
    for input in &targets {
        let output = output_name_for(input, ext);
        match add::create_archive(&output, std::slice::from_ref(input), &spec, quiet) {
            Ok(code) => worst = worst.max(code),
            Err(e) => {
                eprintln!("error: {}: {e:#}", input.display());
                worst = worst.max(exit::code_for_anyhow(&e));
            }
        }
    }
    Ok(worst)
}

/// `<dir-or-stem>.<ext>` next to the input.
fn output_name_for(input: &Path, ext: &str) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = if input.is_dir() {
        input
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("archive")
    } else {
        input
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("archive")
    };
    parent.join(format!("{stem}.{ext}"))
}

/// `bx` — identical to `x` (extract every listed archive).
pub fn extract(args: &ExtractArgs, quiet: bool) -> Result<i32> {
    extract_cmd::run(args, false, quiet)
}

/// `bt` — identical to `t` (test every listed archive).
pub fn test(args: &TestArgs, quiet: bool) -> Result<i32> {
    test_cmd::run(args, quiet)
}
