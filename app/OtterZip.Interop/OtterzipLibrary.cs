// OtterZip.Interop — managed facade over the native library.

using System;
using OtterZip.Interop.Native;

namespace OtterZip.Interop;

/// <summary>
/// Entry point for library initialization. Call <see cref="Initialize"/> once at app startup.
/// </summary>
public static class OtterzipLibrary
{
    /// <summary>
    /// The C-ABI contract version these managed bindings were generated
    /// against — MUST match <c>ABI_VERSION</c> in <c>otterzip-ffi/src/lib.rs</c>.
    /// Every FFI struct is trailing-append versioned, so a native DLL built at
    /// a different ABI would be read at the wrong size and silently corrupt
    /// options/reports. Bump this in lockstep whenever the FFI contract changes.
    /// </summary>
    public const uint ExpectedAbiVersion = 9;

    private static readonly System.Threading.Lock s_initLock = new();
    private static bool s_initialized;

    public static void Initialize()
    {
        // Locked: a plain bool check let two early concurrent callers both
        // run native Init (benign today, but cheap to make correct).
        lock (s_initLock)
        {
            if (s_initialized) return;
            // Fail fast on an ABI mismatch (e.g. a partial build that updated
            // the app but not otterzip_ffi.dll) — otherwise every struct is
            // marshaled at the wrong size and extraction runs on garbage
            // options with NO error (INT-H1).
            uint nativeAbi = NativeMethods.AbiVersion();
            if (nativeAbi != ExpectedAbiVersion)
            {
                throw new OtterzipException(
                    $"OtterZip native ABI mismatch: this build expects v{ExpectedAbiVersion} " +
                    $"but otterzip_ffi reports v{nativeAbi}. The installation is inconsistent — reinstall OtterZip.");
            }
            int rc = NativeMethods.Init();
            if (rc != 0)
            {
                throw new OtterzipException(rc, NativeMethods.LastErrorMessage() ?? "otterzip_init failed");
            }
            s_initialized = true;
        }
    }

    public static void Shutdown()
    {
        lock (s_initLock)
        {
            if (!s_initialized) return;
            NativeMethods.Shutdown();
            s_initialized = false;
        }
    }

    public static string? Version => NativeMethods.VersionString();

    public static uint AbiVersion => NativeMethods.AbiVersion();
}
