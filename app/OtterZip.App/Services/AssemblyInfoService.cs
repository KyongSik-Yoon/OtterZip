using System;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Reflection;

namespace OtterZip.App.Services;

/// <summary>
/// Reads assembly identity metadata for the About panel — version, build
/// date (from informational version), and the optional <c>GitHash</c>
/// MSBuild property if it was injected into <see cref="AssemblyMetadataAttribute"/>.
///
/// Centralized so InfoSection doesn't reach for reflection directly.
/// </summary>
public static class AssemblyInfoService
{
    private static readonly Assembly s_appAssembly = typeof(AssemblyInfoService).Assembly;

    /// <summary>
    /// Semantic version string, e.g. "0.1.0". Falls back to "0.0.0" if
    /// the assembly is missing AssemblyVersion (shouldn't happen in
    /// production builds).
    /// </summary>
    public static string Version
    {
        get
        {
            var v = s_appAssembly.GetName().Version;
            return v is null ? "0.0.0" : $"{v.Major}.{v.Minor}.{v.Build}";
        }
    }

    /// <summary>
    /// Short git commit hash (~7 chars) when the build pipeline injected
    /// <c>&lt;AssemblyMetadata Include="GitHash" Value="abc1234" /&gt;</c>.
    /// Returns "dev" otherwise so the About panel still reads sensibly
    /// during local F5 runs.
    /// </summary>
    public static string GitHash => GetMetadata("GitHash") ?? "dev";

    /// <summary>
    /// Build date when injected via the same MSBuild metadata path. Falls
    /// back to the assembly file's last write time so we always have
    /// *something* to show.
    /// </summary>
    public static string BuildDate
    {
        get
        {
            var injected = GetMetadata("BuildDate");
            if (!string.IsNullOrEmpty(injected)) return injected;
            try
            {
                // BaseDirectory is AOT-safe (IL3000 unlike Assembly.Location).
                // We fall back to the executable's mtime as a coarse proxy.
                var exePath = Path.Combine(AppContext.BaseDirectory, "OtterZip.exe");
                if (File.Exists(exePath))
                {
                    return File.GetLastWriteTime(exePath).ToString("yyyy-MM-dd", CultureInfo.InvariantCulture);
                }
            }
            catch (Exception)
            {
                // Single-file / AOT / restricted-FS builds — fall through.
            }
            return "unknown";
        }
    }

    private static string? GetMetadata(string key)
    {
        return s_appAssembly
            .GetCustomAttributes<AssemblyMetadataAttribute>()
            .FirstOrDefault(a => string.Equals(a.Key, key, StringComparison.Ordinal))
            ?.Value;
    }
}
