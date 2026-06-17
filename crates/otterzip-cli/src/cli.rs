//! clap command tree — Bandizip-compatible verbs (`a/x/e/t/l` + batch
//! `bc/bx/bt`) so muscle memory carries over.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "otterzip",
    version,
    about = "OtterZip — fast archive tool (command line)",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Assume "yes" on prompts, suppress progress, and (for extract)
    /// overwrite existing files. Does NOT bypass the create-only guard.
    #[arg(short = 'y', long = "yes", global = true)]
    pub assume_yes: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a new archive from files/folders.
    #[command(name = "a")]
    Add(AddArgs),

    /// Extract with full paths.
    #[command(name = "x")]
    Extract(ExtractArgs),

    /// Extract flattening all paths into the destination.
    #[command(name = "e")]
    ExtractFlat(ExtractArgs),

    /// Test archive integrity.
    #[command(name = "t")]
    Test(TestArgs),

    /// List archive contents.
    #[command(name = "l")]
    List(TestArgs),

    /// Batch-create: one archive per input.
    #[command(name = "bc")]
    BatchCreate(AddArgs),

    /// Batch-extract: extract every listed archive.
    #[command(name = "bx")]
    BatchExtract(ExtractArgs),

    /// Batch-test: test every listed archive.
    #[command(name = "bt")]
    BatchTest(TestArgs),
}

#[derive(Args, Debug)]
pub struct AddArgs {
    /// Output archive path (must not already exist — create-only).
    pub archive: PathBuf,

    /// Files and/or folders to add.
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,

    /// Archive format. Inferred from the output extension if omitted.
    #[arg(long = "format", visible_alias = "fmt")]
    pub format: Option<String>,

    /// Compression level 0–9 (0 = store). Format default if omitted.
    #[arg(short = 'l', long = "level", value_parser = clap::value_parser!(u8).range(0..=9))]
    pub level: Option<u8>,

    /// Encrypt with this password (AES-256).
    #[arg(short = 'p', long = "password")]
    pub password: Option<String>,

    /// Split into volumes of this size (e.g. 100m, 1g).
    #[arg(short = 'v', long = "volume")]
    pub volume: Option<String>,

    /// CPU threads for compression.
    #[arg(short = 't', long = "threads")]
    pub threads: Option<u32>,

    /// Nest a folder's own name as the archive's top-level directory.
    #[arg(long = "storeroot")]
    pub storeroot: bool,
}

#[derive(Args, Debug)]
pub struct ExtractArgs {
    /// Archive(s) to extract.
    #[arg(required = true)]
    pub archives: Vec<PathBuf>,

    /// Destination directory (default: current directory).
    #[arg(short = 'o', long = "output")]
    pub output: Option<PathBuf>,

    /// Password for encrypted archives.
    #[arg(short = 'p', long = "password")]
    pub password: Option<String>,

    /// Overwrite existing files without asking.
    #[arg(long = "aoa", conflicts_with = "skip_existing")]
    pub overwrite_all: bool,

    /// Skip existing files (don't overwrite).
    #[arg(long = "aos")]
    pub skip_existing: bool,
}

#[derive(Args, Debug)]
pub struct TestArgs {
    /// Archive(s) to test / list.
    #[arg(required = true)]
    pub archives: Vec<PathBuf>,

    /// Password for encrypted archives.
    #[arg(short = 'p', long = "password")]
    pub password: Option<String>,
}
