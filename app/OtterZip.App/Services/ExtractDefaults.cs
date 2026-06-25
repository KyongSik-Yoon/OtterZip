using OtterZip.Interop;

namespace OtterZip.App.Services;

/// <summary>
/// Resolves user-configured extraction defaults that aren't carried on the
/// per-extract UI. Currently the file-collision policy
/// (<c>Settings_OverwritePolicy</c>), which the Rust core already honors
/// (Always = overwrite, Never = reject, IfNewer = skip-if-exists); the C#
/// extract call sites historically hardcoded <see cref="OverwritePolicy.Always"/>.
/// </summary>
internal static class ExtractDefaults
{
    /// <summary>
    /// The user's preferred behavior when an output file already exists.
    /// Defaults to <see cref="OverwritePolicy.Rename"/> (keep both, the
    /// Windows/Bandizip-style safe default — never loses data). Existing
    /// installs that previously chose Always/Skip/Never keep that choice;
    /// only the unset default changed. Unknown values clamp to Rename.
    /// </summary>
    public static OverwritePolicy ResolveOverwrite()
    {
        int v = SettingsService.Get<int>("Settings_OverwritePolicy", (int)OverwritePolicy.Rename);
        return v switch
        {
            (int)OverwritePolicy.Never => OverwritePolicy.Never,
            (int)OverwritePolicy.Always => OverwritePolicy.Always,
            (int)OverwritePolicy.IfNewer => OverwritePolicy.IfNewer,
            _ => OverwritePolicy.Rename,
        };
    }
}
