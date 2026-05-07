// OtterZip.Interop — managed facade over the native library.

using System;
using OtterZip.Interop.Native;

namespace OtterZip.Interop;

/// <summary>
/// Entry point for library initialization. Call <see cref="Initialize"/> once at app startup.
/// </summary>
public static class OtterzipLibrary
{
    private static bool s_initialized;

    public static void Initialize()
    {
        if (s_initialized) return;
        int rc = NativeMethods.Init();
        if (rc != 0)
        {
            throw new OtterzipException(rc, NativeMethods.LastErrorMessage() ?? "otterzip_init failed");
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
