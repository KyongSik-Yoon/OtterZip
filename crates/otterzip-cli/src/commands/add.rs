//! `a` — create a NEW archive from files/folders.
//!
//! Create-only: the core has no Update mode (`open()` rejects non-Read),
//! so we guard against an existing target rather than silently
//! overwriting (`Archive::create` would clobber it).

use crate::cli::AddArgs;
use crate::format_map::{codec_for, format_from_output_ext, parse_format, parse_size};
use crate::{exit, progress};
use anyhow::{bail, Context, Result};
use otterzip_core::{Archive, ArchiveFormat, CreateOptions, EncryptionMethod};
use std::path::Path;
use zeroize::Zeroizing;

/// Options gathered from CLI flags, shared by `a` and `bc`.
pub struct CreateSpec<'a> {
    pub format: Option<&'a str>,
    pub level: Option<u8>,
    pub password: Option<&'a str>,
    pub volume: Option<&'a str>,
    pub threads: Option<u32>,
    pub storeroot: bool,
}

pub fn run(args: &AddArgs, quiet: bool) -> Result<i32> {
    let spec = CreateSpec {
        format: args.format.as_deref(),
        level: args.level,
        password: args.password.as_deref(),
        volume: args.volume.as_deref(),
        threads: args.threads,
        storeroot: args.storeroot,
    };
    create_archive(&args.archive, &args.inputs, &spec, quiet)
}

/// Build one archive at `output` from `inputs`. Used by `a` (all inputs
/// → one archive) and `bc` (one input → one archive).
pub fn create_archive(
    output: &Path,
    inputs: &[std::path::PathBuf],
    spec: &CreateSpec,
    quiet: bool,
) -> Result<i32> {
    if output.exists() {
        bail!(
            "'{}' already exists. Adding to an existing archive is not supported yet \
             (create-only) — use a new name or delete the file first.",
            output.display()
        );
    }
    // Split mode never writes `output` itself — it writes `output.001/.002/…`,
    // so the base-name guard above misses a pre-existing volume set and would
    // silently overwrite .001..N (and leave any stale trailing volumes to
    // corrupt reassembly). Refuse if the first segment already exists (CLI-M1).
    if spec.volume.is_some() {
        let mut first = output.as_os_str().to_os_string();
        first.push(".001");
        let first = std::path::PathBuf::from(first);
        if first.exists() {
            bail!(
                "'{}' already exists (a split volume set is present). Use a new name \
                 or delete the existing '<name>.001..NNN' volumes first.",
                first.display()
            );
        }
    }
    let opts = build_options(output, spec)?;

    let mut archive = Archive::create(output, opts)
        .with_context(|| format!("creating '{}'", output.display()))?;
    let mut sink = progress::make_sink(quiet);

    let added = add_all(&mut archive, inputs, spec.storeroot, &mut sink);
    progress::finish(quiet);

    match added {
        Ok(()) => {
            archive.commit().context("finalizing archive")?;
            if !quiet {
                eprintln!("Created {}", output.display());
            }
            Ok(exit::EXIT_OK)
        }
        Err(e) => {
            // Best-effort cleanup of the partial file; surface the real error.
            let _ = archive.rollback();
            Err(e)
        }
    }
}

fn build_options(output: &Path, spec: &CreateSpec) -> Result<CreateOptions> {
    let format = match spec.format {
        Some(f) => parse_format(f)?,
        None => format_from_output_ext(output),
    };
    let mut opts = CreateOptions {
        format,
        ..Default::default()
    };
    match spec.level {
        Some(level) => {
            opts.compression_level = level;
            opts.compression = codec_for(format, level);
        }
        None => opts.compression = codec_for(format, opts.compression_level),
    }
    if let Some(pw) = spec.password {
        opts.password = Some(Zeroizing::new(pw.to_string()));
        opts.encryption = EncryptionMethod::Aes256;
    }
    if let Some(v) = spec.volume {
        opts.volume_size_bytes = Some(parse_size(v)?);
    }
    if let Some(t) = spec.threads {
        opts.thread_count = Some(t);
    }
    Ok(opts)
}

fn add_all<S: otterzip_core::ProgressSink>(
    archive: &mut Archive,
    inputs: &[std::path::PathBuf],
    storeroot: bool,
    sink: &mut S,
) -> Result<()> {
    for input in inputs {
        if !input.exists() {
            bail!("input not found: '{}'", input.display());
        }
        if input.is_dir() {
            let prefix = if storeroot {
                input.file_name().and_then(|n| n.to_str()).unwrap_or("")
            } else {
                ""
            };
            archive
                .add_dir_recursive(input, prefix, Some(&mut *sink))
                .with_context(|| format!("adding folder '{}'", input.display()))?;
        } else {
            let entry = input
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow::anyhow!("non-UTF-8 file name: '{}'", input.display()))?;
            archive
                .add_file(input, entry)
                .with_context(|| format!("adding file '{}'", input.display()))?;
        }
    }
    Ok(())
}

/// Resolve the format a `bc` run should use (explicit flag or default
/// ZIP, since there's no output filename to infer from).
pub fn batch_format(spec: &CreateSpec) -> Result<ArchiveFormat> {
    match spec.format {
        Some(f) => parse_format(f),
        None => Ok(ArchiveFormat::Zip),
    }
}
