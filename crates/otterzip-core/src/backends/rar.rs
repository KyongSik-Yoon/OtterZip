//! RAR backend — EXTRACTION ONLY. Wraps `unrar` 0.5 (RARLAB's UnRAR C++ via
//! `unrar_sys`).
//!
//! Creation is permanently unavailable: the RARLAB UnRAR licence
//! (`unrar_sys-0.5.8/vendor/unrar/license.txt`) forbids using the source to
//! develop a RAR-compatible archiver. `writer.rs`'s `F::Rar => FeatureDisabled`
//! is a LEGAL constraint, not a scope decision. Do not "fix" it.
//!
//! ## Why this backend looks nothing like `sevenz.rs`
//!
//! unrar writes files ITSELF — there is no `Write`-sink API to hand it (`trait
//! ProcessMode` and `Internal::callback` are private, with four sealed impls;
//! open_archive.rs:466-497). And `extract_to` sets `Cmd->DllDestName`, which per
//! extract.cpp:639-644 verbatim "turns off our path processing and app invoking
//! the library cares about everything including safety": it sets
//! `CurConvertSymlinkPaths=false`, disabling unrar's own `LinksToDirs`
//! symlink-escape defence, while `RAR_EXTRACT` still creates REAL reparse points
//! (dll.cpp:387 `Cmd.Test=Operation!=RAR_EXTRACT` → `FileCreateMode=true` →
//! `ExtractSymlink`). `Cmd->SkipSymLinks` is reachable only from the CLI parser
//! (cmddata.cpp:747), so we cannot ask unrar to skip links, and `FileHeader`
//! exposes no `redir_type` so we cannot see them coming either.
//!
//! Therefore unrar only ever writes into an OPAQUE FLAT SLOT inside a scratch
//! directory we own (`e000000000042.part` — zero bytes derived from the entry
//! name). No slot path is a prefix of another, which makes the classic RAR
//! escape — entry 1 turns `sub` into a junction to `C:\Windows\System32`, entry 2
//! writes `sub/evil.dll` through it — UNREPRESENTABLE rather than merely checked.
//! Each slot is then [`harvest`]ed and `fs::rename`d to its validated
//! destination. The scratch lives INSIDE `dest_root`, so that rename is a
//! same-volume metadata op: the entry's bytes cross the disk exactly once.
//!
//! NEVER call the crate's `extract()` / `extract_with_base()`: they pass
//! `DestPathW` and let unrar join the base with its OWN attacker-controlled
//! filename (pathed/all.rs:31 returns `DestName=None`), which bypasses the
//! Phase-7 guard entirely. Hostile names arrive verbatim — a patched fixture
//! with entry name `..\..\a` comes back through the crate unchanged (unrar
//! rewrites `C:` to `C_` but does NOT strip `..`), so
//! [`crate::archive::__resolve_output_path_streaming`] is the only thing
//! standing between the archive and the filesystem.
//!
//! ## Why `extract_entry` never extracts
//!
//! [`RarBackend::extract_entry`] only ever calls the crate's `read()`, whatever
//! the entry's size. `read()` issues RAR_TEST (`ReadToVec::OPERATION =
//! Operation::Test`, open_archive.rs:479), which sets `Cmd.Test` → clears
//! `FileCreateMode` (extract.cpp:783) → unrar creates NOTHING: no file, no
//! reparse point, no hard link, and no `Prealloc` (extract.cpp:773 is gated on
//! `!TestMode`). An earlier revision staged entries over 64 MiB through
//! `extract_to` into a temp file to avoid a large `Vec`; that traded a bounded
//! `Vec` for unrar preallocating the ATTACKER-DECLARED `UnpSize` into `%TEMP%`
//! and for the redirect surface below — and it bought nothing, because
//! `open_entry_stream` (the only caller of consequence) buffers into a `Vec`
//! regardless, so the staging file cost a full-size disk write ON TOP of the
//! `Vec` it was supposed to replace. RAR_TEST is strictly cheaper AND strictly
//! safer.
//!
//! ## Known residual: `FSREDIR_FILECOPY` on the extract-all path
//!
//! `extract_to` sets `Cmd->DllDestName` but leaves `Cmd->ExtrPath` EMPTY
//! (dll.cpp:350; `DestPathW == NULL` because pathed/all.rs:31 hardcodes it).
//! For a RAR5 entry carrying `FHEXTRA_REDIR`, extract.cpp:807 resolves the
//! redirect through `ExtrPrepareName`, which joins `Cmd->ExtrPath` with the
//! `ConvertPath`'d `RedirName` (extract.cpp:1136 + :1220) and NEVER consults
//! `DllDestName`. With `ExtrPath` empty the target is a bare relative path,
//! resolved by the OS against the **process CWD** rather than `dest_root`.
//!
//! * `FSREDIR_HARDLINK` → [`harvest`]'s link-count gate catches it. Closed.
//! * `FSREDIR_FILECOPY` → `ExtractFileCopy` (extract.cpp:1045) copies the
//!   target's bytes into the slot. The result is an ordinary file with link
//!   count 1 and no reparse tag — **filesystem-indistinguishable from an honest
//!   extract**, so no post-write check can see it. OPEN.
//!
//! Bound, honestly: `ConvertPath` (pathfn.cpp:40-88) strips `..`, drive letters
//! and UNC prefixes from `RedirName`, so the reachable set is the CWD subtree,
//! not the filesystem. It is a containment break and a local information
//! disclosure into the user's own output, not arbitrary file read. The real fix
//! is to pin `Cmd->ExtrPath` to the scratch dir: `ConvertPath` then confines the
//! target to ground we own, which holds only opaque `eNNNNNNNNNNNN.part` slots
//! that are renamed away immediately, so both redirect types fail closed with
//! "no link target" — while `DllDestName` keeps winning for `DestFileName`
//! (extract.cpp:635-645), leaving the slot design undisturbed. That needs
//! `RARProcessFileW(handle, RAR_EXTRACT, DestPath=scratch, DestName=slot)`,
//! i.e. an `extract_to_with_base` the crate does not expose (pathed/all.rs:31
//! hardcodes `DestPath=None`) — the same fork the plan already contemplates for
//! `redir_type` and the `#[repr(C, packed)]` shim (plan §6).

use std::cell::{Cell, RefCell};
use std::io::{self, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use unrar::error::{Code, UnrarError, When};
use unrar::{Archive as RarArchive, FileHeader, VolumeInfo};
use zeroize::Zeroizing;

use crate::archive::ExtractWarning;
use crate::backends::{ArchiveBackend, StreamingExtractCtx};
use crate::entry::{Entry, HostOs};
use crate::error::{OtterzipError, Result};
use crate::format::{CompressionMethod, EncryptionMethod};
use crate::options::{ExtractOptions, OverwritePolicy};
use crate::progress::{Progress, ProgressPhase};

// `unrar_sys` 0.5.8's build.rs emits `powrprof` and `shell32` but NOT
// `advapi32`, even though the C++ it vendors needs it: `GetRarDataPath`
// (pathfn.cpp) calls RegOpenKeyExW / RegQueryValueExW / RegCloseKey, `GetRnd`
// (crypt.cpp) calls CryptAcquireContextW / CryptGenRandom / CryptReleaseContext,
// and `Shutdown` (system.cpp) calls OpenProcessToken.
//
// This went unnoticed because a linker only pulls an archive member in to
// resolve a symbol somebody actually referenced: with `unrar` sitting unused in
// Cargo.toml, none of `unrar_sys`'s objects were ever pulled, so their undefined
// imports never surfaced and the workspace linked green. Making the FIRST call
// into unrar — i.e. this backend existing — pulls those objects in and turns the
// omission into 7 × LNK2019 in both `otterzip-cli` and `otterzip-ffi`. Hence an
// explicit directive rather than the "latent fragility" note the plan expected.
//
// Delete only once `unrar_sys` emits it upstream.
#[cfg(windows)]
#[link(name = "advapi32")]
extern "system" {}

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Unix `S_IFMT` / `S_IFLNK` on the high nibble of `file_attr`. unrar derives
/// `FSREDIR_UNIXSYMLINK` from exactly this test (arcread.cpp:307-309).
const UNIX_MODE_MASK: u32 = 0xF000;
const UNIX_MODE_SYMLINK: u32 = 0xA000;

pub(crate) struct RarBackend {
    /// Always the FIRST volume — `first_part()` is applied once at open, so a
    /// right-click on `foo.part03.rar` / `foo.r02` still extracts the whole set.
    path: PathBuf,
    password: Option<Zeroizing<String>>,
    /// ACP-encoded password (see [`password_bytes_for_unrar`]). Zeroized on drop.
    password_acp: Option<Zeroizing<Vec<u8>>>,
    /// Cached metadata snapshot. Unlike 7z, building this costs a full pass over
    /// every header, so it is worth keeping.
    cache: RefCell<Option<Vec<Entry>>>,
    /// `ROADF_ENCHEADERS` — O(1), sampled at open, and valid WITHOUT a password.
    enc_headers: bool,
}
// `Send` is derived: PathBuf + Zeroizing + RefCell + bool. No unrar type is ever
// a field — `OpenArchive` holds `Handle(NonNull<..>)` with no `unsafe impl Send`,
// and every `open_for_*` / `read_header` / `extract_to` takes `self` by value, so
// no handle can outlive a single call. `!Sync` via `RefCell`, matching the
// `ArchiveBackend` contract (backends/mod.rs:53-57).

/// unrar's `RARSetPassword` is the **ANSI** entry point: on Windows it runs
/// `CharToWide` → `MultiByteToWideChar(CP_ACP, ...)` (dll.cpp:457-465,
/// unicode.cpp:85-92), while the crate hands it raw UTF-8 bytes
/// (open_archive.rs:243). On a non-UTF-8 ACP — CP949 (Korean), CP932, CP936 — a
/// non-ASCII password is therefore re-decoded in the ACP and becomes a DIFFERENT
/// password: a guaranteed "wrong password" on archives WinRAR and 7-Zip open
/// fine. The bug is invisible to ASCII-only testing and hits exactly the users
/// the encrypted-RAR support is aimed at.
///
/// We pre-encode to the ACP so unrar's `CharToWide` round-trips back to the real
/// password. Fixed HERE rather than via the app's fusion manifest so that the
/// CLI and `cargo test` are correct too.
#[cfg(windows)]
fn password_bytes_for_unrar(pw: &Zeroizing<String>) -> Result<Zeroizing<Vec<u8>>> {
    use std::ffi::OsStr;
    use std::os::raw::{c_char, c_int, c_uint};
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetACP() -> c_uint;
        fn WideCharToMultiByte(
            cp: c_uint,
            flags: u32,
            wide: *const u16,
            wide_len: c_int,
            mb: *mut c_char,
            mb_len: c_int,
            default_char: *const c_char,
            used_default: *mut c_int,
        ) -> c_int;
    }
    const CP_ACP: c_uint = 0;
    const CP_UTF8: c_uint = 65001;
    /// **Load-bearing, not hygiene.** With `flags == 0` Windows applies
    /// *best-fit mapping*: a character with no exact ACP representation is
    /// silently replaced by a similar-looking one and `used_default` is NOT
    /// set — so the probe below never fires and the mangled password ships.
    /// MEASURED on CP949 (the exact market this function exists for):
    ///
    /// ```text
    ///   "café"   flags=0     -> 63 61 66 65  "cafe"    used_default=0  << SILENT MANGLE
    ///   "café"   flags=0x400 -> 63 61 66 3f  "caf?"    used_default=1  << caught
    ///   "müller" flags=0     -> 6d 75 6c ..  "muller"  used_default=0  << SILENT MANGLE
    ///   "가나"    either      -> b0 a1 b3 aa            used_default=0  << unaffected
    /// ```
    ///
    /// unrar then decodes "cafe" and rejects an archive WinRAR opens fine —
    /// precisely the failure the `used_default` check below claims to prevent.
    /// Genuinely representable CJK encodes identically either way, so the flag
    /// costs the primary case nothing. Both calls MUST pass the same flags or
    /// the measured `need` will not match the second call's output length.
    const WC_NO_BEST_FIT_CHARS: u32 = 0x0000_0400;

    // `CString::new(pw).unwrap()` (open_archive.rs:243) PANICS on an interior NUL.
    if pw.as_bytes().contains(&0) {
        return Err(OtterzipError::InvalidArgument("password contains a NUL byte"));
    }
    // Do NOT reject long passwords. crypt.cpp:57 truncates at
    // `Min(MAXPASSWORD_RAR, MAXPASSWORD) - 1` "For compatibility with existing
    // archives" and WinRAR does the same, so truncating is COMPATIBLE while
    // erroring would refuse archives that open everywhere else.

    // ACP == UTF-8 (fusion manifest / Win11 opt-in) means our bytes already
    // round-trip; ASCII is identical in every Windows ACP. Both skip the
    // conversion — and the first case must be handled here rather than falling
    // through, because `WideCharToMultiByte` rejects a non-NULL
    // `used_default` pointer for CP_UTF8.
    // SAFETY: `GetACP` takes no arguments and cannot fail.
    if unsafe { GetACP() } == CP_UTF8 || pw.is_ascii() {
        return Ok(Zeroizing::new(pw.as_bytes().to_vec()));
    }

    let wide: Vec<u16> = OsStr::new(pw.as_str()).encode_wide().collect();
    let mut used_default: c_int = 0;
    // SAFETY: `wide` is a live `Vec<u16>` and we pass its true length, so the
    // input range is in bounds. A null output pointer with a zero length is the
    // documented "measure the required buffer" form, so nothing is written.
    // `used_default` is a live local. CP_ACP is not UTF-8 on this path (checked
    // above), which is what makes the non-null `used_default` legal.
    let need = unsafe {
        WideCharToMultiByte(
            CP_ACP,
            WC_NO_BEST_FIT_CHARS,
            wide.as_ptr(),
            wide.len() as c_int,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            &mut used_default,
        )
    };
    if need <= 0 {
        return Err(OtterzipError::InvalidArgument(
            "password cannot be encoded in the system ANSI code page",
        ));
    }
    let mut buf = Zeroizing::new(vec![0u8; need as usize]);
    // SAFETY: same input range as the measuring call above, and `buf` was just
    // allocated with exactly the `need` bytes that call asked for — so the write
    // cannot overrun. `need > 0` is checked above.
    unsafe {
        WideCharToMultiByte(
            CP_ACP,
            WC_NO_BEST_FIT_CHARS,
            wide.as_ptr(),
            wide.len() as c_int,
            buf.as_mut_ptr() as *mut c_char,
            need,
            std::ptr::null(),
            &mut used_default,
        );
    }
    // FAIL LOUDLY. Silently sending a mangled password would report "wrong
    // password" for a CORRECT one — the worst possible failure mode for this
    // feature, and one the user cannot diagnose.
    if used_default != 0 {
        return Err(OtterzipError::InvalidArgument(
            "password contains characters the system ANSI code page cannot represent",
        ));
    }
    Ok(buf)
}

/// Non-Windows builds hit unrar's `#elif defined(MBFUNCTIONS)` / `_APPLE`
/// branches of `CharToWide`, which are UTF-8-based — the raw bytes are correct.
#[cfg(not(windows))]
fn password_bytes_for_unrar(pw: &Zeroizing<String>) -> Result<Zeroizing<Vec<u8>>> {
    if pw.as_bytes().contains(&0) {
        return Err(OtterzipError::InvalidArgument("password contains a NUL byte"));
    }
    Ok(Zeroizing::new(pw.as_bytes().to_vec()))
}

impl RarBackend {
    pub(crate) fn open(path: &Path, password: Option<&Zeroizing<String>>) -> Result<Self> {
        // `pathed::construct` does `WideCString::from_os_str(p).expect("Unexpected
        // nul in path")` (pathed/all.rs:8) — a NUL in the path is a PANIC, not a
        // Result, so it has to be screened before the crate ever sees it.
        if path.as_os_str().to_string_lossy().contains('\0') {
            return Err(OtterzipError::InvalidArgument(
                "archive path contains a NUL byte",
            ));
        }
        let password_acp = match password {
            Some(pw) => Some(password_bytes_for_unrar(pw)?),
            None => None,
        };
        // A multi-volume set must be read from part 1, so a later part gets
        // redirected there via `first_part()` (`foo.part04.rar` → `foo.part01.rar`).
        // But the crate's volume regex is loose: it reads the trailing `.<digits>`
        // of an ORDINARY name as a volume number, so a name-only rewrite turns
        // `Backup.2024.rar` → `Backup.0001.rar` and `Season.2.rar` → `Season.1.rar`
        // — opening a sibling the user never clicked (usually missing → a
        // confusing failure; occasionally an UNRELATED archive that happens to
        // exist → the wrong file's contents, silently).
        //
        // So we don't trust the name. We ask the archive itself: `volume_info()`
        // reports whether the opened file IS a subsequent volume. Only then do we
        // redirect. A standalone archive answers `None`/`First` and is opened as
        // clicked, whatever digits its name ends in.
        let opened_at_subsequent_volume = {
            let probe = match password_acp.as_ref() {
                Some(pw) => RarArchive::with_password(path, &pw[..]),
                None => RarArchive::new(path),
            };
            matches!(
                probe.open_for_listing().map(|a| a.volume_info()),
                Ok(VolumeInfo::Subsequent)
            )
        };
        let first = if opened_at_subsequent_volume {
            RarArchive::new(path).first_part()
        } else {
            path.to_path_buf()
        };

        let mut me = Self {
            path: first,
            password: password.cloned(),
            password_acp,
            cache: RefCell::new(None),
            enc_headers: false,
        };
        let enc_headers = me.guard("open", || {
            Ok(me
                .rar()
                .open_for_listing()
                .map_err(|e| me.map_unrar(e, None))?
                .has_encrypted_headers())
        })?;
        me.enc_headers = enc_headers;

        // For `-hp`, `open_for_listing()` returns Ok even with NO password
        // (`RAROpenArchiveEx` runs BEFORE `RARSetPassword`; header decryption is
        // deferred to `RARReadHeaderEx`), so the error lands on the first read
        // instead. Force it now so `Archive::open` fails fast the way 7z does.
        if me.enc_headers {
            let _ = me.metadata_pass()?;
        }
        Ok(me)
    }

    /// The ONLY place a `unrar::Archive` is constructed. `with_password` merely
    /// BORROWS the bytes (`password: Option<&'a [u8]>`, archive.rs:39-43) rather
    /// than copying them, and the `'_` ties that borrow to `&self` — so the
    /// borrow checker proves the `Zeroizing` outlives every archive we build
    /// from it.
    fn rar(&self) -> RarArchive<'_> {
        match self.password_acp.as_ref() {
            Some(pw) => RarArchive::with_password(&self.path, &pw[..]),
            None => RarArchive::new(&self.path),
        }
    }

    /// Wraps every unrar entry point.
    ///
    /// SCOPE — this is about error QUALITY, not process survival: otterzip-ffi
    /// already wraps every call in `catch_unwind_to_error` (ffi/util.rs:18-38) →
    /// `ErrorCode::Generic(-1)`, and the shell extension launches the host EXE
    /// via `ShellExecuteW` rather than hosting the core in-process. What this
    /// buys is a typed error instead of "panic in FFI: called `Option::unwrap()`
    /// on a `None` value", plus coverage for the CLI and the tests, which drive
    /// the core directly with no `catch_unwind` anywhere.
    ///
    /// The crate panics on input we have to treat as ordinary hostile data:
    ///  * `Code::from(c).unwrap()` (open_archive.rs:254/449/556) — the vendored
    ///    library is unrar 7.x and defines `ERAR_LARGE_DICT = 25` (dll.hpp:22),
    ///    returned when a dictionary exceeds `Cmd->WinSizeLimit`, but
    ///    `unrar_sys`'s constants stop at `ERAR_BAD_PASSWORD = 24`. So
    ///    `Code::from(25)` is `None` and `.unwrap()` panics. RAR7 permits 64 GB
    ///    dictionaries: this is reachable with LEGITIMATE archives, not just
    ///    crafted ones.
    ///  * `EntryFlags::from_bits(..).unwrap()` / `ArchiveFlags::from_bits(..)
    ///    .unwrap()` on any unrecognised flag bit.
    ///
    /// Sound because unrar catches its own C++ exceptions at the DLL boundary
    /// (dll.cpp:410-416), so the unwind always begins in Rust. Requires
    /// `panic = "unwind"`, which the release profile pins for exactly this class
    /// of reason.
    fn guard<T>(&self, op: &'static str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(r) => r,
            Err(_) => {
                tracing::warn!(
                    target: "otterzip::security",
                    event = "backend_panic",
                    backend = "rar",
                    op,
                    "unrar rejected the archive by panicking"
                );
                Err(OtterzipError::Corrupted {
                    reason: "RAR backend rejected the archive (unsupported dictionary size — \
                             RAR7 allows dictionaries above the 4 GiB limit — or unrecognised \
                             header flags)"
                        .into(),
                    entry: None,
                })
            }
        }
    }

    fn metadata_pass(&self) -> Result<Vec<Entry>> {
        self.guard("entries", || {
            // `List`, not `ListSplit`: the DLL pools split entries
            // (dll.cpp:234-238 skips `SplitBefore` headers and recurses), so a
            // file spanning three volumes yields ONE header — which is exactly
            // what OtterZip's `Entry` model wants.
            let ar = self
                .rar()
                .open_for_listing()
                .map_err(|e| self.map_unrar(e, None))?;
            let mut out = Vec::new();
            // DO NOT `.filter_map(Result::ok)` here. The iterator latches
            // `damaged = true` on the first `Err` and then returns `None`
            // FOREVER (open_archive.rs:294-315), so swallowing the error turns
            // "multi-volume set is missing part 3" into "a smaller archive" with
            // no error at all: the user extracts 6 of 10 files and is told it
            // succeeded. Never call `force_heal()` — the crate itself documents
            // it as "use at your own risk".
            for item in ar {
                match item {
                    Ok(h) => out.push(header_to_entry(&h)),
                    // The `List` iterator runs an internal `Skip`, so a missing
                    // volume surfaces as `EOpen@Process` during LISTING, not
                    // only during extraction. `map_unrar` turns that into
                    // `MissingVolume`.
                    Err(e) => return Err(self.map_unrar(e, None)),
                }
            }
            Ok(out)
        })
    }

    /// VERIFIED against the shipped fixtures:
    ///
    /// | case                  | signal                    | →                       |
    /// |-----------------------|---------------------------|-------------------------|
    /// | RAR3 `-p`, no pw      | `MissingPassword@Process` | `WrongPassword`         |
    /// | RAR5 `-hp`, no pw     | `MissingPassword@Read`    | `WrongPassword`         |
    /// | RAR5 `-hp`, wrong pw  | `BadPassword@Read`        | `WrongPassword`         |
    /// | RAR5 `-p`, wrong pw   | `BadPassword@Process`     | `WrongPassword`         |
    /// | **RAR3 `-p`, wrong pw** | **`BadData@Process`**   | `WrongPassword` iff guarded |
    /// | genuine corruption    | `BadData@Process`         | `Corrupted`             |
    ///
    /// The last pair is the whole game. RAR3 — and RAR5 without `UsePswCheck`,
    /// and any `BrokenHeader` — has NO password verifier: unrar decrypts to
    /// garbage, the CRC fails, and we get the SAME code as real corruption.
    /// unrar's own UI can tell them apart (extract.cpp:910-926 picks
    /// `UIERROR_CHECKSUMENC` over `UIERROR_CHECKSUM` from `Encrypted &&
    /// (!UsePswCheck || BrokenHeader)`) but the DLL does not export the
    /// distinction. So: a heuristic, guarded on BOTH "we supplied a password"
    /// AND "this entry is encrypted". It fails in the direction that matters — a
    /// corrupted encrypted entry is reported as "wrong password" (annoying, and
    /// recoverable), whereas the inverse would tell a user with a typo that
    /// their archive is destroyed.
    fn map_unrar(&self, e: UnrarError, ctx: Option<&EntryCtx<'_>>) -> OtterzipError {
        use Code::*;
        use When::*;
        let pw_supplied = self.password.is_some();
        let entry_encrypted = ctx.map(|c| c.encrypted).unwrap_or(false);
        let entry = ctx.map(|c| c.path.to_string());

        match (e.code, e.when) {
            // The normal first encounter with a double-clicked encrypted RAR —
            // this must light up the password dialog, not a generic backend error.
            (MissingPassword, _) => self.wrong_pw("no_password_supplied", entry),
            (BadPassword, _) => self.wrong_pw("invalid_password", entry),
            (BadData, Process) if pw_supplied && entry_encrypted => {
                self.wrong_pw("crc_fail_on_encrypted_entry", entry)
            }
            (BadData, Process) => OtterzipError::Corrupted {
                reason: "file CRC error".into(),
                entry,
            },

            // `EOpen@Process` and `EOpen@Open` are the SAME code, disambiguated
            // only by `When`. extract.cpp:614 deliberately refuses to downgrade
            // EOPEN to BAD_PASSWORD, so a missing volume in an ENCRYPTED set
            // stays a volume error — this arm order preserves that precedence.
            (EOpen, Process) => OtterzipError::MissingVolume {
                index: 0,
                // The crate captures the wanted volume name into `user_data.1`
                // and then `process_file_raw` returns only `user_data.0`,
                // dropping it on the floor (open_archive.rs:515-527, 542-560).
                // Reconstruct it from the filename convention instead.
                expected_name: RarArchive::new(&self.path).nth_part(2).map(|p| {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                }),
            },
            (EOpen, _) => OtterzipError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                "could not open RAR archive",
            )),
            (UnknownFormat, Open) => {
                OtterzipError::UnsupportedFormat(Some("unknown RAR encryption".into()))
            }
            (BadArchive, _) | (UnknownFormat, _) => {
                OtterzipError::UnsupportedFormat(Some(e.to_string()))
            }
            (BadData, _) => OtterzipError::Corrupted {
                reason: e.to_string(),
                entry,
            },
            (NoMemory, _) => OtterzipError::BackendError("RAR: out of memory".into()),
            _ => OtterzipError::BackendError(format!("RAR: {e}")),
        }
    }

    /// Mirrors `zip.rs:308-338`: trace the FACT, never the value. `e.to_string()`
    /// is safe to embed anywhere here — `UnrarError`'s `Display` is a fixed
    /// literal table (error.rs:85-110), so it can never carry password bytes.
    fn wrong_pw(&self, reason: &'static str, entry: Option<String>) -> OtterzipError {
        tracing::info!(
            target: "otterzip::security",
            event = "wrong_password",
            backend = "rar",
            reason,
            entry = entry.as_deref().unwrap_or(""),
            "RAR decryption failed"
        );
        OtterzipError::WrongPassword
    }
}

impl ArchiveBackend for RarBackend {
    fn entries(&self) -> Result<Box<dyn Iterator<Item = Result<Entry>> + '_>> {
        if self.cache.borrow().is_none() {
            let v = self.metadata_pass()?;
            *self.cache.borrow_mut() = Some(v);
        }
        let snapshot = self.cache.borrow().as_ref().unwrap().clone();
        Ok(Box::new(snapshot.into_iter().map(Ok)))
    }

    /// Random access does not exist in this format. Per the crate's own README,
    /// entries can "only be processed as a stream, unidirectionally, front to
    /// back", so entry N costs a linear walk skipping 0..N-1 — and on a SOLID
    /// archive each skip re-decompresses. That is why
    /// [`Self::extract_all_streaming`] is mandatory rather than an optimisation.
    fn extract_entry(&self, entry_path: &str, out: &mut dyn Write) -> Result<u64> {
        self.guard("extract_entry", || {
            let mut ar = self
                .rar()
                .open_for_processing()
                .map_err(|e| self.map_unrar(e, None))?;
            loop {
                let at_file = match ar.read_header().map_err(|e| self.map_unrar(e, None))? {
                    Some(f) => f,
                    None => break,
                };
                let (name, encrypted, attr) = {
                    let h = at_file.entry();
                    (rar_path(h), h.is_encrypted(), h.file_attr)
                };
                if name != entry_path {
                    ar = at_file.skip().map_err(|e| self.map_unrar(e, None))?;
                    continue;
                }
                let ectx = EntryCtx {
                    path: entry_path,
                    encrypted,
                };

                // `read()` creates no file, so `harvest` cannot run on this path.
                // Without this check a symlink entry would hand back its TARGET
                // PATH as the entry's "contents" — harmless for test/preview, but
                // a trap the day a write path gets routed through here.
                // INVARIANT: never emit a link body.
                if (attr & UNIX_MODE_MASK) == UNIX_MODE_SYMLINK {
                    return Err(OtterzipError::Corrupted {
                        reason: "entry is a link record; refusing to stream its target".into(),
                        entry: Some(entry_path.into()),
                    });
                }

                // `read()` issues RAR_TEST, not RAR_EXTRACT (`ReadToVec::OPERATION
                // = Operation::Test`, open_archive.rs:479) → `Cmd.Test` is true
                // → `FileCreateMode` is false (extract.cpp:783). unrar therefore
                // creates NOTHING on this path: no file, no reparse point, no
                // hard link, and no `Prealloc` (extract.cpp:773 is gated on
                // `!TestMode`). That is the whole reason every size goes through
                // here — see the module header's "Why `extract_entry` never
                // extracts".
                //
                // RAM is bounded twice over: `Unpack::UnpWriteData` clamps every
                // write to `DestUnpSize - WrittenFileSize` (unpack50.cpp:539-548)
                // so a lying header cannot exceed its own declared size, and
                // `ReadToVec` grows the `Vec` organically via `extend_from_slice`
                // (open_archive.rs:483) rather than reserving the declared size —
                // so a bomb declaring 100 GB over a 1 KB pack costs 1 KB, not
                // 100 GB.
                let (data, _rest) = at_file.read().map_err(|e| self.map_unrar(e, Some(&ectx)))?;
                out.write_all(&data)?;
                return Ok(data.len() as u64);
            }
            Err(OtterzipError::EntryNotFound(entry_path.to_string()))
        })
    }

    fn open_entry_stream(&self, entry_path: &str) -> Result<Box<dyn Read + Send + '_>> {
        let mut buf = Vec::new();
        self.extract_entry(entry_path, &mut buf)?;
        Ok(Box::new(io::Cursor::new(buf)))
    }

    fn extract_all_streaming(&self, ctx: &mut StreamingExtractCtx<'_>) -> Option<Result<()>> {
        Some(self.guard("extract_all", || self.extract_all_inner(ctx)))
    }

    /// MUST override the default (backends/mod.rs:84-92), which walks `entries()`
    /// — on a `-hp` archive that ERRORS with `MissingPassword@Read` rather than
    /// returning `true`, so the C# password dialog would never appear.
    fn is_encrypted_fast(&self) -> Result<bool> {
        if self.enc_headers {
            return Ok(true); // O(1), and readable without a password.
        }
        // An entry-encrypted RAR (`rar a -p`) has PLAINTEXT filenames, so listing
        // succeeds with no password and the per-entry flag is readable.
        for e in self.entries()? {
            if e?.encryption != EncryptionMethod::None {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl RarBackend {
    fn extract_all_inner(&self, ctx: &mut StreamingExtractCtx<'_>) -> Result<()> {
        let meta = self.metadata_pass()?; // cheap; also fails fast on a missing volume
        let total_bytes: u64 = meta.iter().map(|e| e.uncompressed_size).sum();
        let total_entries = u32::try_from(meta.len()).unwrap_or(u32::MAX);
        let cap = ctx.opts.max_total_output_bytes;

        // Gate the DECLARED TOTAL before writing a byte. RAR's declared sizes are
        // authoritative — `Unpack::UnpWriteData` clamps every write to
        // `DestUnpSize - WrittenFileSize` (unpack50.cpp:539-548), unlike a lying
        // tar header — so this is a true bound, and a bomb fails in milliseconds
        // instead of after 15 GiB of disk.
        if cap > 0 && total_bytes > cap {
            return Err(OtterzipError::ZipBombSuspected {
                entry: "<aggregate>".into(),
                ratio: 0,
                limit: ctx.opts.max_total_compression_ratio,
            });
        }

        let scratch = Scratch::new_in(ctx.dest_root)?; // dropped on ANY exit, incl. unwind
        let mut ar = self
            .rar()
            .open_for_processing()
            .map_err(|e| self.map_unrar(e, None))?;
        let mut idx: u32 = 0;

        loop {
            let at_file = match ar.read_header().map_err(|e| self.map_unrar(e, None))? {
                Some(f) => f,
                None => break,
            };
            // Copy the header facts out BEFORE the consuming op: `skip` /
            // `extract_to` / `read` all take `self` by value, so the header is
            // gone afterwards and there is no second chance to ask.
            let (pod, encrypted, declared, is_dir, attr) = {
                let h = at_file.entry();
                (
                    header_to_entry(h),
                    h.is_encrypted(),
                    h.unpacked_size,
                    h.is_directory(),
                    h.file_attr,
                )
            };
            let path_str = pod.path.clone();
            let ectx = EntryCtx {
                path: &path_str,
                encrypted,
            };

            // Cancellation is BETWEEN ENTRIES ONLY. unrar itself can abort
            // mid-entry (returning -1 from `UCM_PROCESSDATA` triggers
            // `ErrHandler.Exit(RARX_USERBREAK)`, rdwrfn.cpp:160-163) but the
            // crate's callback is hardcoded and never returns -1
            // (open_archive.rs:504-535). A single 20 GB entry will not respond to
            // Cancel — a UX note for the progress dialog, unfixable from here.
            let snapshot = Progress {
                bytes_processed: ctx.report.bytes_written,
                bytes_total: total_bytes,
                entries_processed: idx,
                entries_total: total_entries,
                current_entry: Some(path_str.clone()),
                phase: ProgressPhase::Writing,
                elapsed: ctx.start.elapsed(),
                current_entry_bytes_processed: 0,
                current_entry_bytes_total: 0,
            };
            if !ctx.progress.update(&snapshot) {
                return Err(OtterzipError::Canceled); // scratch drop cleans the partial
            }
            idx = idx.saturating_add(1);

            // BOMB GATE, PRE-WRITE. Mandatory, not polish: `check_zip_bomb` is
            // INERT for RAR (`compressed_size == 0` bails at archive.rs:1356),
            // `__copy_capped` cannot wrap a sink that does not exist on this
            // path, and unrar PREALLOCATES the slot to `UnpSize` before writing a
            // byte (extract.cpp:773-780, gated on `PackSize*1024 > UnpSize`) — so
            // a 100 GB entry from a 200 MB pack claims 100 GB of disk before any
            // during-copy cap could fire.
            if !is_dir && cap > 0 && ctx.report.bytes_written.saturating_add(declared) > cap {
                return Err(OtterzipError::ZipBombSuspected {
                    entry: path_str,
                    ratio: 0,
                    limit: ctx.opts.max_total_compression_ratio,
                });
            }
            // With the cap DISABLED (0) that prealloc is otherwise unbounded, so
            // the destination's free space becomes the only remaining floor.
            if !is_dir && declared > free_space(ctx.dest_root)? {
                return Err(OtterzipError::Io(io::Error::new(
                    io::ErrorKind::Other,
                    "entry does not fit in the destination's free space",
                )));
            }
            // Inert today; wired anyway so a future `pack_size` fix lights it up
            // for free.
            if let Some(e) = crate::archive::__check_bomb_for_streaming(&pod, ctx.opts) {
                return Err(e);
            }

            // Traversal check BEFORE unrar is allowed to write. Fail-closed via
            // `skip()`.
            let out_path = match crate::archive::__resolve_output_path_streaming(
                ctx.dest_root,
                &path_str,
                ctx.opts,
                pod.is_directory,
            ) {
                Ok(p) => p,
                Err(orig) => {
                    ar = at_file.skip().map_err(|e| self.map_unrar(e, Some(&ectx)))?;
                    if ctx.opts.block_path_traversal {
                        tracing::warn!(
                            target: "otterzip::security",
                            event = "path_traversal_blocked",
                            entry = %orig,
                            backend = "rar"
                        );
                        return Err(OtterzipError::PathTraversalBlocked(orig));
                    }
                    ctx.report
                        .warnings
                        .push(ExtractWarning::PathTraversalClamped {
                            original: orig,
                            clamped: ctx.dest_root.to_path_buf(),
                        });
                    ctx.report.entries_skipped += 1;
                    continue;
                }
            };

            if is_dir {
                std::fs::create_dir_all(&out_path)?;
                ar = at_file.skip().map_err(|e| self.map_unrar(e, Some(&ectx)))?;
                ctx.report.entries_extracted += 1;
                continue;
            }

            // Advisory symlink pre-filter: catches the UNIX subset cheaply so the
            // common case is never written at all. `file_attr` is
            // attacker-controlled, so this is a fast path, NOT the defence.
            if (attr & UNIX_MODE_MASK) == UNIX_MODE_SYMLINK && !ctx.opts.follow_symlinks {
                ar = at_file.skip().map_err(|e| self.map_unrar(e, Some(&ectx)))?;
                ctx.report.warnings.push(ExtractWarning::SymlinkSkipped {
                    path: path_str,
                    target: String::new(),
                });
                ctx.report.entries_skipped += 1;
                continue;
            }

            // THE WRITE — into a path with ZERO attacker-derived bytes.
            let slot = scratch.slot();
            ar = at_file
                .extract_to(&slot)
                .map_err(|e| self.map_unrar(e, Some(&ectx)))?;

            // Post-write. THE real symlink gate.
            let written = match harvest(&slot)? {
                Harvest::Link | Harvest::Dir => {
                    ctx.report.warnings.push(ExtractWarning::SymlinkSkipped {
                        path: path_str,
                        target: String::new(),
                    });
                    ctx.report.entries_skipped += 1;
                    force_remove(&slot); // never rename it, never open it
                    continue;
                }
                Harvest::File(n) => n,
            };
            // The pre-gate trusted the header; verify reality. This is the
            // invariant the whole pre-gate rests on, so assert it rather than
            // assume it.
            if written > declared {
                force_remove(&slot);
                return Err(OtterzipError::Corrupted {
                    reason: "unrar wrote past the declared unpacked size".into(),
                    entry: Some(path_str),
                });
            }
            if cap > 0 && ctx.report.bytes_written.saturating_add(written) > cap {
                force_remove(&slot);
                return Err(OtterzipError::ZipBombSuspected {
                    entry: path_str,
                    ratio: 0,
                    limit: ctx.opts.max_total_compression_ratio,
                });
            }

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let final_path = match apply_overwrite_policy(out_path, ctx.opts)? {
                Destination::Write(p) => p,
                Destination::Skip => {
                    force_remove(&slot);
                    ctx.report.entries_skipped += 1;
                    continue;
                }
            };
            // SINGLE-VOLUME RENAME — the whole point of keeping the scratch
            // inside `dest_root`. `std::fs::rename` → `MoveFileExW(
            // MOVEFILE_REPLACE_EXISTING)`, a metadata op. The entry's bytes were
            // written exactly once.
            rename_or_copy(&slot, &final_path)?;

            if let Some(payload) = ctx.motw_payload {
                if let Err(e) = crate::motw::write_zone_identifier(&final_path, payload) {
                    tracing::warn!(
                        target: "otterzip::motw",
                        path = %final_path.display(),
                        error = %e,
                        "MOTW propagation skipped (RAR extract)"
                    );
                }
            }
            ctx.report.bytes_written += written;
            ctx.report.entries_extracted += 1;
        }
        Ok(())
    }

}

/// Header facts copied out BEFORE the consuming op. `extract_to` / `read` /
/// `skip` take `self` by value, so the header is gone afterwards.
struct EntryCtx<'a> {
    path: &'a str,
    encrypted: bool,
}

/// `FileHeader.filename` is a Windows `PathBuf` built from a UTF-16 buffer via
/// `WideCString::from_ptr_truncate(.., 1024)` (open_archive.rs:649-651), and RAR
/// stores `\` on Windows-created archives — a patched fixture named `a\b\c.t`
/// comes back with the backslashes intact.
///
/// Normalising is required by CLAUDE.md rule 3, and independently for
/// correctness: `detect_root_layout` (archive.rs:628) splits on '/', and
/// `__validate_component` REJECTS an embedded backslash — so an unnormalised
/// `a\b` would be misreported as *traversal*.
///
/// Note that `from_ptr_truncate` silently truncates names past 1024 wide chars.
/// The truncated name is what `entries()` emits AND what `extract_entry` keys on,
/// so the round trip stays self-consistent (a truncated name simply won't match a
/// real entry → `EntryNotFound`, which is fail-closed). This function must stay a
/// pure deterministic function of the header for that invariant to hold.
fn rar_path(h: &FileHeader) -> String {
    h.filename.to_string_lossy().replace('\\', "/")
}

fn header_to_entry(h: &FileHeader) -> Entry {
    Entry {
        path: rar_path(h),
        is_directory: h.is_directory(),
        // Only the UNIX subset is inferable, and unrar derives
        // FSREDIR_UNIXSYMLINK the same way (arcread.cpp:307-309). RAR5
        // WINSYMLINK / JUNCTION come from the FHEXTRA_REDIR extra field and are
        // INVISIBLE through this wrapper. Advisory only — `harvest` is the real
        // gate. (0x81a4 = 0o100644 regular, 0xa1ff = 0o120777 symlink.)
        is_symlink: (h.file_attr & UNIX_MODE_MASK) == UNIX_MODE_SYMLINK,
        uncompressed_size: h.unpacked_size,
        // An honest 0: `FileHeader` exposes seven fields and `pack_size` is not
        // among them (open_archive.rs:582-590). Same as solid 7z. This is what
        // makes `check_zip_bomb` inert (archive.rs:1356) and the declared-size
        // gates in `extract_all_inner` necessary.
        compressed_size: 0,
        // No RAR variant exists in the enum; `Unknown` is the 0xFFFF_FFFF escape
        // hatch. Adding one means a schema.md §2 change plus an ABI bump.
        compression: CompressionMethod::Unknown,
        encryption: if h.is_encrypted() {
            EncryptionMethod::Aes256
        } else {
            EncryptionMethod::None
        },
        crc32: Some(h.file_crc),
        modified: dos_time_to_system_time(h.file_time),
        // Present in the C struct, dropped by the wrapper.
        accessed: None,
        created: None,
        attributes: h.file_attr,
        // `Archive::set_comments` is a documented no-op in the crate.
        comment: None,
        host_os: HostOs::Unknown,
    }
}

/// RAR packs mtime into ONE u32 in MS-DOS format — high half date, low half time
/// (`D->FileTime = hd->mtime.GetDos()`, dll.cpp:274; the bit layout is at
/// timefn.cpp:199-206). It is NOT a Unix epoch value, and like ZIP's it is local
/// time with no zone recorded. Decoded the same way the ZIP backends decode
/// theirs so `Entry.modified` stays consistent across formats; the civil-date
/// arithmetic is Howard Hinnant's, copied from `backends/zip.rs` for the same
/// reason `lenient_zip.rs` copies it.
fn dos_time_to_system_time(file_time: u32) -> Option<SystemTime> {
    let dos_date = (file_time >> 16) as u16;
    let dos_time = (file_time & 0xFFFF) as u16;
    if dos_date == 0 && dos_time == 0 {
        return None;
    }
    let year = ((dos_date >> 9) as i32) + 1980;
    let month = u32::from((dos_date >> 5) & 0x0f);
    let day = u32::from(dos_date & 0x1f);
    let hour = u32::from(dos_time >> 11);
    let minute = u32::from((dos_time >> 5) & 0x3f);
    let second = u32::from((dos_time & 0x1f) * 2);

    let days = days_from_civil(year, month, day)?;
    let secs = i64::from(days) * 86_400
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second);
    let secs_u64 = u64::try_from(secs).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(secs_u64))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i32> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let civil_year = if month <= 2 { year - 1 } else { year };
    let era = civil_year.div_euclid(400);
    let yoe = u32::try_from(civil_year.rem_euclid(400)).ok()?;
    let month_offset = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * month_offset + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era.checked_mul(146_097)?
        .checked_add(i32::try_from(doe).ok()?)?
        .checked_sub(719_468)
}

/// RAII scratch, created INSIDE `dest_root` so that `harvest` → `fs::rename` is a
/// same-volume metadata op rather than a byte copy — and so a 40 GB extract to
/// `D:` does not need 40 GB free on `C:`.
struct Scratch {
    dir: tempfile::TempDir,
    next: Cell<u64>,
}

impl Scratch {
    fn new_in(dest_root: &Path) -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix(".otterzip-rar-")
            .suffix(".tmp")
            .tempdir_in(dest_root)?;
        Ok(Self {
            dir,
            next: Cell::new(0),
        })
    }

    /// **The security crux.** The slot name contains ZERO bytes derived from
    /// `header.filename`, so nothing in the archive can steer where unrar writes
    /// — `DestNameW` is exactly this path (dll.cpp:381-384: no join, no concat,
    /// no normalisation step for an attacker to influence).
    ///
    /// And every slot is a flat SIBLING, so no slot path is a prefix of another,
    /// which structurally kills the classic RAR escape:
    ///
    /// ```text
    ///     entry 1: `sub`       -> junction to C:\Windows\System32
    ///     entry 2: `sub/x.dll` -> written THROUGH the junction
    /// ```
    ///
    /// because entry 2 goes to `e000000000001.part`, never
    /// `e000000000000.part/x.dll`. Extracting to the final path would need a
    /// per-entry `fs::canonicalize()` re-check to survive that; this design does
    /// not.
    ///
    /// Bonus: ~19 ASCII chars, so a 300-deep RAR tree never blows MAX_PATH during
    /// DECOMPRESSION — only the final rename touches the long path, and it does
    /// so against the `\\?\`-prefixed `dest_root` that `canonical_root` already
    /// returns (archive.rs:1380-1388).
    fn slot(&self) -> PathBuf {
        let n = self.next.get();
        self.next.set(n + 1);
        self.dir.path().join(format!("e{n:012}.part"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // unrar stamps `Arc.FileHead.FileAttr` onto our slot (extract.cpp:996),
        // so a RAR entry marked READONLY / HIDDEN / SYSTEM makes `TempDir`'s own
        // `remove_dir_all` fail with ACCESS_DENIED and leak the scratch — which
        // the FIRST WinRAR-created archive containing a read-only file would hit.
        // Clearing the attributes here works because `TempDir`'s `Drop` runs
        // immediately afterwards (fields drop after the struct's own `Drop` body).
        clear_readonly_recursive(self.dir.path());
    }
}

/// Best-effort: every failure just leaves the scratch for `TempDir` to try.
fn clear_readonly_recursive(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        // `symlink_metadata` never traverses. Both branches below depend on that:
        // clearing READONLY on a reparse point would follow it and clear the bit
        // on the LINK TARGET, and recursing into one would walk the real
        // C:\Windows.
        let Ok(md) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if is_reparse_point(&md) {
            continue;
        }
        clear_readonly(&path, &md);
        if md.is_dir() {
            clear_readonly_recursive(&path);
        }
    }
}

/// Windows-only by design, not by accident. The bit being defused is
/// `FILE_ATTRIBUTE_READONLY`, which unrar stamps onto our slot from the entry
/// header (extract.cpp:996) and which blocks `DeleteFileW`. Unix has no such
/// interaction — unlinking depends on the *containing directory's* write bit,
/// which we own — and `Permissions::set_readonly(false)` there means mode 0o666,
/// which would both publish a world-writable file and strip a directory's
/// execute bit, breaking the very delete this is meant to enable.
///
/// The caller must have ruled out reparse points: `set_permissions` follows
/// them, so on a junction this would clear the READONLY bit on the link target.
#[cfg(windows)]
fn clear_readonly(path: &Path, md: &std::fs::Metadata) {
    let mut perms = md.permissions();
    if perms.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(windows))]
fn clear_readonly(_path: &Path, _md: &std::fs::Metadata) {}

enum Harvest {
    File(u64),
    Link,
    Dir,
}

/// Everything checkable only AFTER unrar has written.
///
/// This — not the header check — is the primary link defence, because
/// `FileHeader` exposes no `redir_type` and you cannot reach around it to
/// `unrar_sys` for one either: the C header is `#pragma pack(1)` (dll.hpp:4)
/// while `unrar_sys` declares a plain `#[repr(C)]`, so every field from `CmtBuf`
/// on is off by 4 bytes and you would read garbage.
///
/// If the slot is a link, opening it would read THROUGH it: an entry pointing at
/// C:\Windows\System32\config\SAM would copy that file's bytes into the user's
/// output under an innocent name. Arbitrary file read. Fail closed.
///
/// ## Two independent gates, because a link is not always a reparse point
///
/// 1. **Reparse tag** — covers `FSREDIR_UNIXSYMLINK` / `WINSYMLINK` /
///    `JUNCTION`, which `ExtractSymlink` materialises as real reparse points.
/// 2. **Hard-link count** — covers `FSREDIR_HARDLINK`, which materialises as an
///    ORDINARY DIRECTORY ENTRY with **no tag at all**. `symlink_metadata` cannot
///    see it: `is_reparse_point` is false, `is_dir` is false, so gate 1 alone
///    returns `Harvest::File(target_len)` and the slot — a live alias for a file
///    outside `dest_root` — gets renamed into the user's output. `extract_to`
///    leaves `Cmd->ExtrPath` EMPTY (dll.cpp:350; `DestPathW == NULL`), and
///    `ExtrPrepareName` builds the redirect target as `ExtrPath + RedirName`
///    (extract.cpp:1136 + :1220) while never consulting `DllDestName` — so
///    `CreateHardLink(slot, <process CWD>/RedirName)` (hardlinks.cpp:13) is what
///    actually runs. `Cmd->SkipSymLinks` would not help even if it were
///    reachable: extract.cpp:657 covers only the three symlink types.
///
/// A freshly created file always has a link count of 1, so gate 2 cannot
/// false-positive on an honest extract.
///
/// **Known residual: `FSREDIR_FILECOPY`.** It is a genuine byte copy (link count
/// 1, no tag), so neither gate sees it, and it is filesystem-indistinguishable
/// from an honest extract. See the module header for the bound and the fix.
fn harvest(slot: &Path) -> Result<Harvest> {
    let md = std::fs::symlink_metadata(slot)?; // does NOT traverse
    if is_reparse_point(&md) {
        return Ok(Harvest::Link);
    }
    if md.is_dir() {
        return Ok(Harvest::Dir);
    }
    if is_hard_link(slot) {
        return Ok(Harvest::Link);
    }
    Ok(Harvest::File(md.len()))
}

/// `true` when `slot` shares its data with another name on the volume — i.e.
/// unrar resolved an `FSREDIR_HARDLINK` redirect against the process CWD and
/// aliased our slot onto it. Fail-closed: an unreadable slot counts as a link,
/// because we just watched unrar create it and "cannot open the thing I am about
/// to publish" is not a state worth publishing through.
#[cfg(windows)]
fn is_hard_link(slot: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    // `std::os::windows::fs::MetadataExt::number_of_links` is still unstable
    // (`windows_by_handle`), so go to the API it wraps.
    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    #[repr(C)]
    #[derive(Default)]
    struct Filetime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        file_attributes: u32,
        creation_time: Filetime,
        last_access_time: Filetime,
        last_write_time: Filetime,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(
            handle: *mut std::ffi::c_void,
            info: *mut ByHandleFileInformation,
        ) -> i32;
    }

    // FILE_READ_ATTRIBUTES is the minimum right this query needs and, unlike
    // GENERIC_READ, it is granted even on a file whose DACL denies us the
    // contents — so a hardlink aimed at something unreadable is still *detected*
    // rather than mistaken for an I/O error. OPEN_REPARSE_POINT so we can never
    // traverse; BACKUP_SEMANTICS so a directory handle is legal too.
    let opened = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(slot);
    let Ok(f) = opened else {
        return true; // fail closed
    };
    let mut info = ByHandleFileInformation::default();
    // SAFETY: `f` owns a live handle for the duration of the call, and `info` is
    // a live local matching the BY_HANDLE_FILE_INFORMATION layout the API fills.
    let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle().cast(), &mut info) };
    if ok == 0 {
        return true; // fail closed
    }
    info.number_of_links > 1
}

/// Unix hard links are a real thing, but this backend never reaches the
/// `FSREDIR_HARDLINK` path off Windows in a way that escapes `dest_root`
/// differently from the symlink case, and `std::fs::Metadata::nlink` is
/// available there if it is ever needed. Kept as a stub so `harvest` stays one
/// function on both platforms.
#[cfg(not(windows))]
fn is_hard_link(slot: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(slot).map_or(true, |md| md.nlink() > 1)
}

fn is_reparse_point(md: &std::fs::Metadata) -> bool {
    if md.file_type().is_symlink() {
        return true;
    }
    // Belt and braces: `FileType::is_symlink` only covers the SYMLINK and
    // MOUNT_POINT reparse tags, and any other tag (APPEXECLINK, DEDUP, a
    // third-party filter) is still something we refuse to open or rename.
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// unrar stamps the entry's `FileAttr` onto the slot (extract.cpp:996), so a
/// READONLY entry defeats a naive `remove_file` with ACCESS_DENIED.
fn force_remove(path: &Path) {
    let Ok(md) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !is_reparse_point(&md) {
        clear_readonly(path, &md);
    }
    if md.is_dir() {
        // A real directory: `is_dir()` is false for a reparse point read through
        // `symlink_metadata`, so this never recurses into a junction's target.
        let _ = std::fs::remove_dir_all(path);
    } else if std::fs::remove_file(path).is_err() {
        // A junction / directory symlink is neither a plain file nor `is_dir()`;
        // `remove_dir` unlinks the reparse point without touching the target.
        let _ = std::fs::remove_dir(path);
    }
}

/// A mount point INSIDE `dest_root` makes the rename fail with
/// ERROR_NOT_SAME_DEVICE even though the scratch was created under it.
fn rename_or_copy(from: &Path, to: &Path) -> Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if is_cross_device(&e) => {
            std::fs::copy(from, to)?;
            let _ = std::fs::remove_file(from);
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Matched on the raw OS code rather than `io::ErrorKind::CrossesDevices`: that
/// variant only stabilised in 1.85 and the workspace pins
/// `rust-version = "1.80"`.
fn is_cross_device(e: &io::Error) -> bool {
    #[cfg(windows)]
    const CROSS_DEVICE: i32 = 17; // ERROR_NOT_SAME_DEVICE
    #[cfg(not(windows))]
    const CROSS_DEVICE: i32 = 18; // EXDEV
    e.raw_os_error() == Some(CROSS_DEVICE)
}

/// Free bytes available to the calling user (quota-aware), used as the floor on
/// unrar's prealloc when the byte cap is disabled.
#[cfg(windows)]
fn free_space(dir: &Path) -> Result<u64> {
    use std::os::raw::c_int;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            dir: *const u16,
            free_to_caller: *mut u64,
            total: *mut u64,
            total_free: *mut u64,
        ) -> c_int;
    }
    let mut wide: Vec<u16> = dir.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut free: u64 = 0;
    // SAFETY: `wide` is a live, NUL-terminated UTF-16 buffer. `free` is a live
    // local of the ULARGE_INTEGER width the API writes. The two `total` outputs
    // are documented as optional and ignored when null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(OtterzipError::Io(io::Error::last_os_error()));
    }
    Ok(free)
}

/// No prealloc floor off Windows: unrar's `Prealloc` is a `SetFilePointer` +
/// `SetEndOfFile` pair guarded by `#ifdef _WIN_ALL`, so there is nothing to
/// bound. `u64::MAX` makes the caller's comparison a no-op rather than
/// pretending to a defence we do not have here.
#[cfg(not(windows))]
fn free_space(_dir: &Path) -> Result<u64> {
    Ok(u64::MAX)
}

/// Where an entry ends up once [`OverwritePolicy`] has had its say.
enum Destination {
    Write(PathBuf),
    Skip,
}

/// Parity with `sevenz.rs:223-243` / `tar_family.rs:243-261`, except that the
/// decision necessarily lands AFTER the write: unrar offers no sink, so the
/// bytes are already in the scratch slot by the time we know the final name.
/// Nothing has been published to `dest_root` yet, so a `Skip` or an error still
/// leaves the destination untouched.
fn apply_overwrite_policy(out_path: PathBuf, opts: &ExtractOptions) -> Result<Destination> {
    if !out_path.exists() {
        return Ok(Destination::Write(out_path));
    }
    Ok(match opts.overwrite {
        OverwritePolicy::Never => {
            return Err(OtterzipError::Io(io::Error::new(
                io::ErrorKind::AlreadyExists,
                out_path.display().to_string(),
            )))
        }
        OverwritePolicy::Always => Destination::Write(out_path),
        // Keep both: divert this entry to `name (2).ext`.
        OverwritePolicy::Rename => {
            Destination::Write(crate::archive::unique_extract_path(&out_path))
        }
        OverwritePolicy::IfNewer | OverwritePolicy::AskCallback => Destination::Skip,
    })
}

/// Unit tests for the three things `tests/rar_extract.rs` structurally cannot
/// reach: a private fn (`password_bytes_for_unrar`), a private post-write gate
/// (`harvest`), and the sink `extract_entry` writes into — which has to be OUR
/// sink for the staging question to be observable at all.
#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("rar")
            .join(name)
    }

    // -----------------------------------------------------------------
    // harvest — the FSREDIR_HARDLINK gate
    // -----------------------------------------------------------------

    /// A hard link is an ORDINARY DIRECTORY ENTRY: no reparse tag, `is_dir()`
    /// false. The reparse-tag check alone therefore classified it
    /// `Harvest::File(target_len)` and `extract_all_inner` renamed the slot —
    /// a live alias for a file outside `dest_root` — straight into the user's
    /// output, where a write through it rewrites the original.
    ///
    /// Reachable because `extract_to` leaves `Cmd->ExtrPath` empty (dll.cpp:350),
    /// so extract.cpp:807's `ExtrPrepareName(Arc, RedirName, NameExisting)`
    /// resolves an `FSREDIR_HARDLINK` redirect against the PROCESS CWD and
    /// hardlinks our slot onto it (hardlinks.cpp:13). `Cmd->SkipSymLinks` does
    /// not cover hard links even when reachable (extract.cpp:657), and
    /// `FileHeader` exposes no `redir_type`, so this gate is the only one.
    #[test]
    fn harvest_rejects_a_hard_link() {
        let td = tempfile::tempdir().unwrap();
        let outside = td.path().join("secret.key");
        std::fs::write(&outside, b"a secret, outside dest_root").unwrap();

        let slot = td.path().join("e000000000000.part");
        // Stands in for CreateHardLink(slot, <CWD>/RedirName) — std::fs::hard_link
        // IS CreateHardLinkW on Windows, so this is the same object unrar makes.
        std::fs::hard_link(&outside, &slot).unwrap();

        assert!(
            matches!(harvest(&slot).unwrap(), Harvest::Link),
            "a hard link must never harvest as a plain file: it would be renamed \
             into dest_root as a live alias for {}",
            outside.display()
        );
    }

    /// The other half: the gate must not false-positive. A freshly created file
    /// always has a link count of 1, so an honest extract is untouched.
    #[test]
    fn harvest_accepts_an_ordinary_file() {
        let td = tempfile::tempdir().unwrap();
        let slot = td.path().join("e000000000001.part");
        std::fs::write(&slot, b"hello").unwrap();

        assert!(
            matches!(harvest(&slot).unwrap(), Harvest::File(5)),
            "the link-count gate broke ordinary extraction"
        );
    }

    // -----------------------------------------------------------------
    // password — the CP_ACP round trip
    // -----------------------------------------------------------------

    /// Decode exactly the way unrar will: `RARSetPassword` -> `CharToWide` ->
    /// `MultiByteToWideChar(CP_ACP, 0, ..)` (unicode.cpp:91, VERIFIED verbatim).
    /// Whatever this returns is the password unrar actually tries.
    #[cfg(windows)]
    fn decode_as_unrar_will(bytes: &[u8]) -> String {
        use std::os::raw::{c_char, c_int, c_uint};

        #[link(name = "kernel32")]
        extern "system" {
            fn MultiByteToWideChar(
                cp: c_uint,
                flags: u32,
                mb: *const c_char,
                mb_len: c_int,
                wide: *mut u16,
                wide_len: c_int,
            ) -> c_int;
        }
        const CP_ACP: c_uint = 0;

        // SAFETY: `bytes` is a live slice passed with its true length; the null
        // output pointer with a zero length is the documented "measure" form.
        let need = unsafe {
            MultiByteToWideChar(
                CP_ACP,
                0,
                bytes.as_ptr().cast(),
                bytes.len() as c_int,
                std::ptr::null_mut(),
                0,
            )
        };
        assert!(need > 0, "ACP decode failed for {bytes:02x?}");
        let mut wide = vec![0u16; need as usize];
        // SAFETY: same input range; `wide` holds exactly the `need` units the
        // measuring call asked for.
        unsafe {
            MultiByteToWideChar(
                CP_ACP,
                0,
                bytes.as_ptr().cast(),
                bytes.len() as c_int,
                wide.as_mut_ptr(),
                need,
            );
        }
        String::from_utf16_lossy(&wide)
    }

    /// **The invariant, not the implementation.** `password_bytes_for_unrar` may
    /// return bytes, or it may refuse — but if it returns bytes, feeding them
    /// through unrar's own decoder MUST reproduce the password the user typed.
    ///
    /// This is what `WC_NO_BEST_FIT_CHARS` buys. With `flags = 0` Windows
    /// best-fit-maps an unrepresentable character to a look-alike and leaves
    /// `used_default` CLEAR, so the `used_default` probe never fires and the
    /// mangled bytes ship. MEASURED on CP949: `"café"` -> `63 61 66 65` =
    /// `"cafe"`, `used_default = 0`. unrar then decodes `"cafe"` and reports a
    /// wrong password for a CORRECT one, on an archive WinRAR opens fine — the
    /// exact failure the function exists to prevent, and one the user cannot
    /// diagnose because retyping the same correct password fails identically.
    ///
    /// ACP-independent by construction: on CP949 `café` refuses and `암호`
    /// round-trips; on CP1252 the reverse; on a UTF-8 ACP everything
    /// round-trips through the identity fast path. Every ACP proves the same
    /// invariant, so this needs no CI locale pinning.
    #[cfg(windows)]
    #[test]
    fn password_bytes_are_never_silently_mangled() {
        for pw in [
            "ascii-only",
            "암호",              // CP949-representable
            "パスワード",          // CP932-representable
            "café",              // U+00E9 best-fit maps to 'e' on CP949
            "müller!",           // U+00FC best-fit maps to 'u' on CP949
            "café2024",          // the reported scenario, verbatim
            "日本語",             // CP932/CP936-representable
            "🦦",                // astral: representable almost nowhere
        ] {
            let z = Zeroizing::new(pw.to_owned());
            match password_bytes_for_unrar(&z) {
                Ok(bytes) => {
                    let seen = decode_as_unrar_will(&bytes);
                    assert_eq!(
                        seen, pw,
                        "SILENT MANGLE: {pw:?} was encoded as {bytes:02x?}, which unrar \
                         decodes back as {seen:?}. A correct password would be reported \
                         WRONG. Pass WC_NO_BEST_FIT_CHARS to both WideCharToMultiByte \
                         calls so the used_default probe can fire."
                    );
                }
                // Refusing loudly is the designed outcome for anything the ACP
                // cannot carry. That is a diagnosable error; a mangle is not.
                Err(OtterzipError::InvalidArgument(_)) => {}
                Err(other) => panic!("{pw:?} produced an untyped failure: {other:?}"),
            }
        }
    }

    // -----------------------------------------------------------------
    // extract_entry — never on the RAR_EXTRACT path
    // -----------------------------------------------------------------

    /// Scratch dirs sitting DIRECTLY in the system temp dir. `extract_all`'s
    /// scratch lives in `dest_root`, and the integration tests' `dest_root`s are
    /// nested inside a `tempdir()`, so a non-recursive scan here only ever sees
    /// the one `stage_to_sink` used to create.
    fn temp_scratches() -> std::collections::BTreeSet<PathBuf> {
        let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else {
            return std::collections::BTreeSet::new();
        };
        rd.flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(".otterzip-rar-"))
            })
            .collect()
    }

    /// Observes the staging file WHILE it exists.
    ///
    /// This has to be our own sink: the old `stage_to_sink` created the scratch,
    /// let unrar write the entry into it, and only then `io::copy`'d it out — so
    /// the scratch is live exactly during the `write` calls, and gone by the time
    /// `extract_entry` returns (`Scratch`'s `Drop`). Snapshotting before/after
    /// therefore cannot see it; the sink can.
    #[derive(Default)]
    struct ScratchSpy {
        seen: Vec<PathBuf>,
        bytes: Vec<u8>,
    }

    impl Write for ScratchSpy {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.seen.extend(temp_scratches());
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// `bigdecl.rar` declares `UNP_SIZE = 100 MiB` over 11 real bytes — a pure
    /// header lie, which is the whole point: the declared size is
    /// attacker-chosen, and it used to be what `extract_entry` branched on.
    /// Above 64 MiB it staged through `extract_to` (RAR_EXTRACT), which
    /// (a) writes the entry into `%TEMP%` — on C:, even when extracting to D: —
    /// (b) lets unrar `Prealloc(UnpSize)` claim the DECLARED size instantly
    /// (extract.cpp:773-780), and (c) opens the FSREDIR_HARDLINK / FILECOPY
    /// redirect surface, which `Archive::test` — the one API a user invokes
    /// *because they do not trust the archive* — reached with no cap at all.
    ///
    /// `read()` issues RAR_TEST: `FileCreateMode` false, so unrar creates
    /// nothing and preallocates nothing, and RAM tracks the REAL data because
    /// `ReadToVec` grows organically. Hence: no size branch, no staging, at any
    /// declared size.
    #[test]
    fn extract_entry_never_stages_through_temp() {
        let before = temp_scratches();

        let backend = RarBackend::open(&fixture("bigdecl.rar"), None).unwrap();
        assert_eq!(
            backend.entries().unwrap().next().unwrap().unwrap().uncompressed_size,
            100 * 1024 * 1024,
            "fixture must declare far more than it holds, or this proves nothing"
        );

        let mut spy = ScratchSpy::default();
        let n = backend.extract_entry("VERSION", &mut spy).unwrap();

        // The declared 100 MiB is a lie; the real entry is 11 bytes and that is
        // all that may be produced, in RAM or anywhere else.
        assert_eq!(n, 11);
        assert_eq!(spy.bytes, b"unrar-0.4.0");

        let staged: Vec<_> = spy.seen.iter().filter(|p| !before.contains(*p)).collect();
        assert!(
            staged.is_empty(),
            "extract_entry staged a 100 MiB-declared entry through %TEMP%: {staged:?}"
        );
    }
}
