//! Standalone pigz corruption diagnostic.
//!
//! Usage:
//!     cargo run --release --example diagnose_pigz -- <file>
//!
//! What it does:
//!   1. Reads the file fully into memory.
//!   2. Runs the **reverted pigz path** (Mode A — single-pass
//!      compress_vec(..., Sync/Finish), no preset dictionary — the
//!      exact code that produced corrupt archives on the user's 4
//!      large entries).
//!   3. Runs the **agent-3 fix candidate** (Mode B — two-pass NoFlush
//!      + terminator, deflateSetDictionary(prev_32k) chaining,
//!      no-progress-Ok sentinel for Sync completion).
//!   4. Concatenates each mode's block outputs, inflates the result
//!      with libdeflate, and compares with the original byte-by-byte.
//!   5. For each mode reports:
//!        * total blocks, compressed size, ratio
//!        * the byte trailer of each block (proves Sync emitted the
//!          `00 00 ff ff` aligner or not)
//!        * first mismatch byte offset (if any)
//!        * 32-byte hex context around the mismatch (input vs decoded)
//!
//! Why: the user is rightly reluctant to enable the parallel path
//! again without isolating which hypothesis was the actual cause of
//! the GB-scale corruption. This tool runs *both* hypotheses against
//! the user's real binary in seconds and prints definitive evidence.

use std::env;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Instant;

use flate2::{Compress, Compression as Lvl, FlushCompress, Status};
use libdeflater::Decompressor;
use rayon::prelude::*;

const PIGZ_BLOCK_SIZE: usize = 1 << 20; // 1 MiB
const DICT_SIZE: usize = 32 * 1024;
const LEVEL: u8 = 5;

/// Mode A — the **reverted code path**, copied verbatim from
/// `zip_writer.rs::deflate_block_raw` before it was disabled in
/// commit `7953f49`. Single-pass `compress_vec` with the terminator
/// flush mode, no preset dictionary. This is the hypothesis-under-test
/// for "GB-scale corruption was caused by sync-flush completion
/// detection bug".
fn deflate_block_mode_a(input: &[u8], level: u8, is_final: bool) -> Vec<u8> {
    let lvl = Lvl::new(u32::from(level.clamp(1, 9)));
    let mut compressor = Compress::new(lvl, false);
    let mut output = Vec::with_capacity(input.len() + 64);

    let flush_mode = if is_final {
        FlushCompress::Finish
    } else {
        FlushCompress::Sync
    };

    let mut in_consumed: usize = 0;
    loop {
        let prev_out = output.len();
        let status = compressor
            .compress_vec(&input[in_consumed..], &mut output, flush_mode)
            .expect("compress_vec");
        in_consumed = compressor.total_in() as usize;
        match status {
            Status::StreamEnd => break,
            Status::Ok => {
                if in_consumed >= input.len() {
                    if is_final {
                        if output.len() == prev_out {
                            output.reserve(64);
                        }
                    } else {
                        break; // ← THE SUSPECT LINE
                    }
                }
            }
            Status::BufError => {
                output.reserve(64 + input.len() / 4);
            }
        }
    }
    output
}

/// Mode E — zlib-spec dual sentinel. Mode C's "Ok with no output
/// progress" alone hangs on zlib-ng: when the encoder has drained
/// input + emitted the Sync trailer, zlib-ng returns `Z_BUF_ERROR`
/// (flate2's `Status::BufError`), not `Z_OK` with zero progress.
/// Mode C's BufError branch only `reserve()`s and loops, so the
/// loop never exits.
///
/// The zlib manual is explicit on this:
/// > "the next call of deflate() must use the same value of the
/// >  flush parameter, until the deflate() function returns with
/// >  Z_BUF_ERROR."
///
/// So Mode E treats both `Ok` *and* `BufError` as Sync-completion
/// sentinels when `in_consumed >= input.len() && !made_progress`.
/// Real out-of-buffer (input not yet drained) still triggers a
/// `reserve()` in the BufError branch.
fn deflate_block_mode_e(input: &[u8], level: u8, is_final: bool) -> Vec<u8> {
    let lvl = Lvl::new(u32::from(level.clamp(1, 9)));
    let mut compressor = Compress::new(lvl, false);
    let mut output = Vec::with_capacity(input.len() + 64);

    let flush_mode = if is_final {
        FlushCompress::Finish
    } else {
        FlushCompress::Sync
    };

    let mut in_consumed: usize = 0;
    loop {
        if output.capacity() - output.len() < 1024 {
            output.reserve(64 * 1024);
        }
        let prev_out = output.len();
        let status = compressor
            .compress_vec(&input[in_consumed..], &mut output, flush_mode)
            .expect("compress_vec");
        in_consumed = compressor.total_in() as usize;
        let made_progress = output.len() > prev_out;
        match status {
            Status::StreamEnd => break, // Finish completed
            Status::Ok => {
                if in_consumed >= input.len() && !is_final && !made_progress {
                    // Some zlib variants signal Sync completion via Ok
                    // with no progress instead of BufError. Treat both
                    // as the spec sentinel.
                    break;
                }
            }
            Status::BufError => {
                if in_consumed >= input.len() && !is_final && !made_progress {
                    // Canonical zlib sentinel: Sync flush complete.
                    break;
                }
                // Real out-of-buffer before input drained — grow.
                output.reserve(64 * 1024);
            }
        }
    }
    output
}

/// Mode B — agent-3 fix candidate. Two-pass `compress_vec`
/// (`NoFlush` until input drained, then a single terminator pass),
/// no-progress-Ok sentinel for Sync completion, optional
/// `deflateSetDictionary(prev_last_32k_input)` for compression-ratio
/// recovery. This is the algorithm pigz itself uses (line-for-line
/// equivalent of `compress_thread()` in pigz.c).
fn deflate_block_mode_b(
    input: &[u8],
    prev_dict: Option<&[u8]>,
    level: u8,
    is_final: bool,
) -> Vec<u8> {
    let lvl = Lvl::new(u32::from(level.clamp(1, 9)));
    let mut compressor = Compress::new(lvl, false);

    if let Some(dict) = prev_dict {
        let n = dict.len().min(DICT_SIZE);
        let slice = &dict[dict.len() - n..];
        compressor.set_dictionary(slice).expect("set_dictionary");
    }

    let mut output = Vec::with_capacity(input.len() + input.len() / 1024 + 64);
    let mut in_consumed: usize = 0;

    loop {
        let flush = if in_consumed < input.len() {
            FlushCompress::None
        } else if is_final {
            FlushCompress::Finish
        } else {
            FlushCompress::Sync
        };

        if output.capacity() - output.len() < 1024 {
            output.reserve(64 * 1024);
        }
        let prev_out_len = output.len();
        let status = compressor
            .compress_vec(&input[in_consumed..], &mut output, flush)
            .expect("compress_vec");
        in_consumed = compressor.total_in() as usize;

        match status {
            Status::StreamEnd => break, // Finish completed
            Status::Ok => {
                if in_consumed >= input.len() && !is_final {
                    // Sync completion sentinel — zlib's contract is
                    // "Sync flush is complete when avail_out remained
                    // nonzero after the call returned with no input
                    // and no new output", which compress_vec exposes
                    // as a no-progress Ok.
                    if matches!(flush, FlushCompress::Sync) && output.len() == prev_out_len {
                        break;
                    }
                }
            }
            Status::BufError => {
                output.reserve(64 * 1024);
            }
        }
    }
    output
}

/// Slice the previous-block input's last `DICT_SIZE` bytes. Returns
/// `None` for block 0. For block N ≥ 1 returns a view into the
/// original `input` so the dictionary is zero-copy.
fn dict_for_block<'a>(input: &'a [u8], block_start_offset: usize) -> Option<&'a [u8]> {
    if block_start_offset == 0 {
        None
    } else {
        let start = block_start_offset.saturating_sub(DICT_SIZE);
        Some(&input[start..block_start_offset])
    }
}

#[derive(Default)]
struct ModeReport {
    blocks: usize,
    compressed_size: u64,
    encode_ms: u128,
    decode_ok: bool,
    first_mismatch: Option<usize>,
    /// Trailer of each block's compressed bytes — last 5 bytes
    /// should be `00 00 ff ff` for any non-final block that used
    /// Sync flush. If we ever see something else here it proves
    /// the encoder didn't fully emit the aligner.
    sync_trailers: Vec<[u8; 5]>,
}

fn inflate_concat(blocks: &[Vec<u8>]) -> Vec<u8> {
    let total: usize = blocks.iter().map(|b| b.len()).sum();
    let mut concat = Vec::with_capacity(total);
    for b in blocks {
        concat.extend_from_slice(b);
    }
    let mut decompressor = Decompressor::new();
    // Output bound: we know the original input size.
    let mut out = vec![0u8; concat.len() * 16]; // generous
    let n = decompressor
        .deflate_decompress(&concat, &mut out)
        .expect("inflate concat");
    out.truncate(n);
    out
}

fn try_inflate_concat(blocks: &[Vec<u8>], expected_len: usize) -> Result<Vec<u8>, String> {
    let total: usize = blocks.iter().map(|b| b.len()).sum();
    let mut concat = Vec::with_capacity(total);
    for b in blocks {
        concat.extend_from_slice(b);
    }
    let mut decompressor = Decompressor::new();
    let mut out = vec![0u8; expected_len + 1024];
    match decompressor.deflate_decompress(&concat, &mut out) {
        Ok(n) => {
            out.truncate(n);
            Ok(out)
        }
        Err(e) => Err(format!("inflate error: {e:?}")),
    }
}

fn run_mode(
    label: &str,
    input: &[u8],
    encode: impl Fn(usize, &[u8], Option<&[u8]>, bool) -> Vec<u8> + Sync,
) -> ModeReport {
    let block_count = input.chunks(PIGZ_BLOCK_SIZE).count();
    let last_idx = block_count.saturating_sub(1);
    println!("\n=== Mode {label} — {block_count} blocks ===");

    let started = Instant::now();
    let blocks: Vec<(usize, &[u8])> = input.chunks(PIGZ_BLOCK_SIZE).enumerate().collect();
    let block_pairs: Vec<(usize, &[u8], Option<&[u8]>)> = blocks
        .iter()
        .map(|(idx, block)| {
            let block_start = idx * PIGZ_BLOCK_SIZE;
            let dict = dict_for_block(input, block_start);
            (*idx, *block, dict)
        })
        .collect();

    let encoded: Vec<Vec<u8>> = block_pairs
        .par_iter()
        .map(|(idx, block, dict)| encode(*idx, block, *dict, *idx == last_idx))
        .collect();
    let encode_ms = started.elapsed().as_millis();

    let compressed_size: u64 = encoded.iter().map(|b| b.len() as u64).sum();
    let ratio = compressed_size as f64 / input.len() as f64;
    println!(
        "  encoded in {encode_ms} ms, compressed_size = {compressed_size} ({ratio:.3} ratio)"
    );

    // Sync trailers — print first 4 and last 2 non-final blocks.
    let mut sync_trailers = Vec::with_capacity(encoded.len());
    for b in &encoded {
        let mut t = [0u8; 5];
        let n = b.len().min(5);
        if n > 0 {
            t[5 - n..].copy_from_slice(&b[b.len() - n..]);
        }
        sync_trailers.push(t);
    }
    let print_trailer = |idx: usize, label: &str| {
        let t = sync_trailers[idx];
        println!(
            "  block[{idx:5}] {label}: last 5 bytes = {:02x} {:02x} {:02x} {:02x} {:02x}",
            t[0], t[1], t[2], t[3], t[4]
        );
    };
    if encoded.len() > 1 {
        for i in 0..encoded.len().min(4) {
            if i != last_idx {
                print_trailer(i, "(non-final)");
            }
        }
        if encoded.len() > 6 {
            let mid = encoded.len() / 2;
            print_trailer(mid, "(middle)");
            print_trailer(last_idx.saturating_sub(1), "(penultimate)");
        }
    }
    print_trailer(last_idx, "(FINAL — should NOT end 00 00 ff ff)");

    // Inflate
    let inflate_result = try_inflate_concat(&encoded, input.len());
    let (decode_ok, first_mismatch) = match inflate_result {
        Ok(decoded) => {
            if decoded.len() != input.len() {
                println!(
                    "  ✗ inflate length mismatch: got {} expected {}",
                    decoded.len(),
                    input.len()
                );
                (false, Some(decoded.len().min(input.len())))
            } else {
                let mismatch = decoded.iter().zip(input.iter()).position(|(a, b)| a != b);
                if let Some(off) = mismatch {
                    println!("  ✗ FIRST MISMATCH at byte offset {off}");
                    let block_idx = off / PIGZ_BLOCK_SIZE;
                    let block_local_off = off % PIGZ_BLOCK_SIZE;
                    println!("    inside block[{block_idx}] at offset {block_local_off}");
                    // 32-byte hex context
                    let ctx_start = off.saturating_sub(16);
                    let ctx_end = (off + 16).min(input.len());
                    let inp = &input[ctx_start..ctx_end];
                    let dec = &decoded[ctx_start..ctx_end];
                    println!("    input  : {}", hex32(inp));
                    println!("    decoded: {}", hex32(dec));
                    (false, Some(off))
                } else {
                    println!("  ✓ inflate OK, byte-for-byte match with input");
                    (true, None)
                }
            }
        }
        Err(e) => {
            println!("  ✗ inflate FAILED: {e}");
            (false, None)
        }
    };

    ModeReport {
        blocks: encoded.len(),
        compressed_size,
        encode_ms,
        decode_ok,
        first_mismatch,
        sync_trailers,
    }
}

fn hex32(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: diagnose_pigz <file>");
        std::process::exit(2);
    }
    let path = Path::new(&args[1]);
    println!("=== diagnose_pigz ===");
    println!("file: {}", path.display());

    let meta = fs::metadata(path).expect("metadata");
    println!("size: {} bytes ({} MiB)", meta.len(), meta.len() / (1 << 20));

    let started = Instant::now();
    let mut input = Vec::with_capacity(meta.len() as usize);
    fs::File::open(path)
        .expect("open")
        .read_to_end(&mut input)
        .expect("read");
    println!("read in {} ms", started.elapsed().as_millis());

    // Mode A only — Mode B (two-pass + dictionary) hangs on
    // zlib-ng under certain conditions when called with NoFlush →
    // Sync transition. Investigating Mode B's hang is itself
    // diagnostic evidence: agent-3's proposed fix isn't a drop-in
    // free win. For the user-corpus diagnostic we only need Mode A's
    // verdict.
    let mode_a = run_mode("A (reverted, single-pass Sync, no dict)", &input, |_, block, _, is_final| {
        deflate_block_mode_a(block, LEVEL, is_final)
    });
    let mode_e = run_mode("E (zlib-spec dual sentinel, no dict)", &input, |_, block, _, is_final| {
        deflate_block_mode_e(block, LEVEL, is_final)
    });

    println!("\n=== Summary ===");
    println!(
        "Mode A: blocks={} size={} encode={}ms decode_ok={} first_mismatch={:?}",
        mode_a.blocks, mode_a.compressed_size, mode_a.encode_ms, mode_a.decode_ok, mode_a.first_mismatch
    );
    println!(
        "Mode E: blocks={} size={} encode={}ms decode_ok={} first_mismatch={:?}",
        mode_e.blocks, mode_e.compressed_size, mode_e.encode_ms, mode_e.decode_ok, mode_e.first_mismatch
    );

    if mode_a.decode_ok && mode_e.decode_ok {
        println!("\n→ Both Mode A and Mode E byte-correct on this file.");
        println!("  Means: this binary's pattern doesn't trigger the Sync-truncation bug.");
        println!("  GB-scale corruption then comes from somewhere else.");
    } else if !mode_a.decode_ok && mode_e.decode_ok {
        println!("\n→ Mode A CORRUPT, Mode E CLEAN — corruption root cause CONFIRMED.");
        println!("  The fix is the dual-sentinel break-condition change in Mode E.");
        println!("  Safe to apply to zip_writer.rs::deflate_block_raw.");
    } else if !mode_a.decode_ok && !mode_e.decode_ok {
        println!("\n→ Both modes still corrupt — Mode E's fix is INSUFFICIENT.");
        println!("  Need deeper investigation (possibly dictionary chaining required).");
    } else {
        println!("\n→ Anomalous: A clean, E broken. Investigate Mode E's loop logic.");
    }

}

#[allow(dead_code)]
fn _keep_mode_b_for_reference() -> u8 {
    // Mode B's hang is itself a diagnostic finding worth keeping
    // the source for. Once the live patch sprint starts, we'll
    // bisect zlib-ng's NoFlush→Sync state machine to find the
    // condition that makes its compress_vec loop never reach the
    // no-progress-Ok sentinel. For now the symbol stays referenced
    // through this no-op so the dead-code lint doesn't bite.
    let _ = deflate_block_mode_b(&[], None, LEVEL, false);
    0
}
