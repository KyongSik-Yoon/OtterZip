//! `l` — list archive contents to stdout.

use crate::cli::TestArgs;
use crate::commands::open_archive;
use crate::{exit, output};
use anyhow::Result;
use std::path::Path;

pub fn run(args: &TestArgs) -> Result<i32> {
    let mut worst = exit::EXIT_OK;
    let multi = args.archives.len() > 1;
    for (i, archive) in args.archives.iter().enumerate() {
        if multi {
            if i > 0 {
                println!();
            }
            println!("== {} ==", archive.display());
        }
        if let Err(e) = list_one(archive, args.password.as_deref()) {
            eprintln!("error: {}: {e:#}", archive.display());
            worst = worst.max(exit::code_for_anyhow(&e));
        }
    }
    Ok(worst)
}

fn list_one(path: &Path, password: Option<&str>) -> Result<()> {
    let (archive, _merged) = open_archive(path, password)?;
    let mut entries = Vec::new();
    for entry in archive.entries()? {
        entries.push(entry?);
    }
    output::print_entries(&entries);
    Ok(())
}
