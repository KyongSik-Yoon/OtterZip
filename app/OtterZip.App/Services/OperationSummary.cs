using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text;

namespace OtterZip.App.Services;

/// <summary>
/// Formats the one-line completion detail shown after a compress / extract
/// job. The "simple + fast" concept wants the speed and the win to be
/// *felt*, so compress shows <c>archive size · NN%↓ · NN MB/s</c>. The
/// numeric units (MB, MB/s, %, ↓) are language-neutral, so no .resw entry
/// is required.
/// </summary>
internal static class OperationSummary
{
    /// <summary>
    /// Compress detail: archive size, the space saved (only when it actually
    /// shrank), and throughput (only when the run was long enough to mean
    /// something). e.g. <c>12.3 MB · 45%↓ · 180 MB/s</c>.
    /// </summary>
    public static string Compress(ulong originalBytes, ulong archiveBytes, TimeSpan elapsed)
    {
        var sb = new StringBuilder(FormatSize(archiveBytes));
        if (originalBytes > 0 && archiveBytes < originalBytes)
        {
            double pct = (1.0 - ((double)archiveBytes / originalBytes)) * 100.0;
            sb.Append(Sep).Append(string.Format(CultureInfo.CurrentCulture, "{0:0}%↓", pct));
        }
        AppendSpeed(sb, originalBytes, elapsed);
        return sb.ToString();
    }

    /// <summary>
    /// Extract detail: bytes written + throughput. e.g.
    /// <c>1.2 GB · 320 MB/s</c>. No ratio (extraction doesn't compress).
    /// </summary>
    public static string Extract(ulong bytesWritten, TimeSpan elapsed)
    {
        var sb = new StringBuilder(FormatSize(bytesWritten));
        AppendSpeed(sb, bytesWritten, elapsed);
        return sb.ToString();
    }

    /// <summary>Sum of file sizes under <paramref name="paths"/> (files +
    /// recursive folder contents). Best-effort; unreadable entries skipped.</summary>
    public static ulong TotalInputBytes(IEnumerable<string> paths)
    {
        ulong total = 0;
        foreach (string p in paths)
        {
            try
            {
                if (File.Exists(p))
                {
                    total += (ulong)new FileInfo(p).Length;
                }
                else if (Directory.Exists(p))
                {
                    foreach (string f in Directory.EnumerateFiles(p, "*", SearchOption.AllDirectories))
                    {
                        try { total += (ulong)new FileInfo(f).Length; }
                        catch (IOException) { /* skip locked/vanished file */ }
                    }
                }
            }
            catch (Exception)
            {
                // Best-effort: a bad path just doesn't contribute to the total.
            }
        }
        return total;
    }

    private const string Sep = "  ·  ";

    private static void AppendSpeed(StringBuilder sb, ulong bytes, TimeSpan elapsed)
    {
        // Sub-50ms runs make the rate huge + meaningless (and divide noisily).
        if (bytes == 0 || elapsed.TotalSeconds < 0.05)
        {
            return;
        }
        double bytesPerSecond = bytes / elapsed.TotalSeconds;
        sb.Append(Sep).Append(FormatSize((ulong)bytesPerSecond)).Append("/s");
    }

    public static string FormatSize(ulong bytes)
    {
        const ulong KB = 1024;
        const ulong MB = KB * 1024;
        const ulong GB = MB * 1024;
        // CurrentCulture on purpose: user-facing number, so the decimal
        // separator should match the locale. (Was misleadingly named `inv`.)
        var culture = CultureInfo.CurrentCulture;
        if (bytes >= GB) { return string.Format(culture, "{0:0.##} GB", bytes / (double)GB); }
        if (bytes >= MB) { return string.Format(culture, "{0:0.##} MB", bytes / (double)MB); }
        if (bytes >= KB) { return string.Format(culture, "{0:0.##} KB", bytes / (double)KB); }
        return string.Format(culture, "{0} B", bytes);
    }
}
