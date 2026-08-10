// OtterZip for Linux — failure → user-readable headline.
//
// Same contract as the WinUI ErrorMessages: map known error codes onto the
// localized catalogue, keep anything unmapped verbatim so unexpected failures
// stay debuggable. Two differences from the Windows implementation:
//
//   * Lookup goes through the Linux `Strings` helper rather than MRT.
//   * "Disk full" is ENOSPC (errno 28), not the Win32 pair (0x70 / 0x27).
//     .NET surfaces errno in the low word of HResult on Unix the same way it
//     surfaces the Win32 code on Windows, so the shape of the check carries
//     over — only the constant changes.

using System;
using System.IO;
using OtterZip.Interop;

namespace OtterZip.App.Services;

internal static class ErrorMessages
{
    /// <summary>ENOSPC — "No space left on device" on Linux and macOS alike.</summary>
    private const int NoSpaceLeftOnDevice = 28;

    /// <summary>EROFS — read-only file system; also a "cannot write here" case.</summary>
    private const int ReadOnlyFileSystem = 30;

    /// <summary>EDQUOT — over the user's disk quota. Same story as ENOSPC.</summary>
    private const int DiskQuotaExceeded = 122;

    public static string Localize(Exception ex)
    {
        ArgumentNullException.ThrowIfNull(ex);
        try
        {
            if (ex is OtterzipException oz)
            {
                return oz.ErrorCode switch
                {
                    // App-synthesized message that is already a finished
                    // localized string — show it verbatim.
                    OtterzipErrorCodes.AlreadyLocalized => oz.Message,
                    OtterzipErrorCodes.WrongPassword => Strings.Get("Error_WrongPassword"),
                    OtterzipErrorCodes.PathTraversal => Strings.Get("Error_PathTraversal"),
                    OtterzipErrorCodes.ZipBomb => Strings.Get("Error_ZipBomb"),
                    OtterzipErrorCodes.FeatureDisabled => Strings.Get("Error_FeatureDisabled"),
                    _ => Strings.Get("Error_ArchiveFailed"),
                };
            }
            if (ex is IOException io
                && (io.HResult & 0xFFFF) is NoSpaceLeftOnDevice or DiskQuotaExceeded or ReadOnlyFileSystem)
            {
                return Strings.Get("Error_DiskFull");
            }
            if (ex is UnauthorizedAccessException)
            {
                return Strings.Get("Error_OperationFailed");
            }
        }
        catch (Exception)
        {
            // Resource lookup must never mask the real failure.
        }
        return string.IsNullOrEmpty(ex.Message)
            ? Strings.Get("Error_OperationFailed")
            : ex.Message;
    }
}
