using System;
using System.IO;
using Microsoft.Windows.ApplicationModel.Resources;
using OtterZip.Interop;

namespace OtterZip.App.Services;

/// <summary>
/// Maps operation failures onto localized, user-readable one-liners for
/// JobCards / status surfaces. Before this, <c>JobQueue.MarkError</c> put
/// the RAW native message on the card — the Rust core's English string
/// ("Could not find EOCD…") in every locale (pre-1.0 UX review finding).
///
/// Known error codes get a clean localized headline; anything unmapped
/// keeps the original message so unexpected failures stay debuggable.
/// </summary>
internal static class ErrorMessages
{
    private static readonly Lazy<ResourceLoader> s_strings =
        new(() => new ResourceLoader());

    // ERROR_DISK_FULL / ERROR_HANDLE_DISK_FULL (Win32 → HResult low word).
    private const int DiskFull = 0x70;
    private const int HandleDiskFull = 0x27;

    public static string Localize(Exception ex)
    {
        ArgumentNullException.ThrowIfNull(ex);
        try
        {
            if (ex is OtterzipException oz)
            {
                return oz.ErrorCode switch
                {
                    // App-synthesized message that's already a finished localized
                    // string — show it verbatim (e.g. "spanned 7z — v1.1 예정").
                    OtterzipErrorCodes.AlreadyLocalized => oz.Message,
                    OtterzipErrorCodes.WrongPassword => Get("Error_WrongPassword"),
                    OtterzipErrorCodes.PathTraversal => Get("Error_PathTraversal"),
                    OtterzipErrorCodes.ZipBomb => Get("Error_ZipBomb"),
                    OtterzipErrorCodes.FeatureDisabled => Get("Error_FeatureDisabled"),
                    // Corrupt / truncated / unsupported-method / core IO all
                    // surface as one honest headline instead of raw English.
                    _ => Get("Error_ArchiveFailed"),
                };
            }
            if (ex is IOException io
                && (io.HResult & 0xFFFF) is DiskFull or HandleDiskFull)
            {
                return Get("Error_DiskFull");
            }
        }
        catch (Exception)
        {
            // Resource lookup must never mask the real failure.
        }
        // Managed-side / unexpected failure — keep the detail.
        return string.IsNullOrEmpty(ex.Message)
            ? GetSafe("Error_OperationFailed")
            : ex.Message;
    }

    private static string Get(string key) => s_strings.Value.GetString(key + "/Text");

    private static string GetSafe(string key)
    {
        try
        {
            return Get(key);
        }
        catch (Exception)
        {
            return "Error";
        }
    }
}
