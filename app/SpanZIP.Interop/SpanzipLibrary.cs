// SpanZIP.Interop — managed facade over the native library.

using System;
using SpanZIP.Interop.Native;

namespace SpanZIP.Interop;

/// <summary>
/// Entry point for library initialization. Call <see cref="Initialize"/> once at app startup.
/// </summary>
public static class SpanzipLibrary
{
    private static bool s_initialized;

    public static void Initialize()
    {
        if (s_initialized) return;
        int rc = NativeMethods.Init();
        if (rc != 0)
        {
            throw new SpanzipException(rc, NativeMethods.LastErrorMessage() ?? "spanzip_init failed");
        }
        s_initialized = true;
    }

    public static void Shutdown()
    {
        if (!s_initialized) return;
        NativeMethods.Shutdown();
        s_initialized = false;
    }

    public static string? Version => NativeMethods.VersionString();

    public static uint AbiVersion => NativeMethods.AbiVersion();
}
