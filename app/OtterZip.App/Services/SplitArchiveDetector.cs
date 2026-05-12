using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Text.RegularExpressions;

namespace OtterZip.App.Services;

/// <summary>
/// Detect the three common multi-volume archive layouts produced by
/// mainstream tools, **before** the Rust core's single-volume backend
/// trips on a missing EOCD / mid-stream truncation. The OtterZip 1.0
/// release does not yet ship native cross-volume readers (Rust's
/// <c>open_multi</c> is discovery-only), so the host UI uses this
/// detector to pick between three outcomes:
///
///   1. <see cref="SplitKind.None"/> — single-volume; proceed normally.
///   2. <see cref="SplitKind.RawByteSplit"/> — a contiguous
///      <c>name.zip.001..NNN</c> set produced by WinRAR / 7-Zip's "split
///      to volumes" feature without container-level spanning. These can
///      be losslessly recombined by byte-concatenation, which OtterZip
///      handles via a temp-file then routes through the regular extract
///      path.
///   3. <see cref="SplitKind.Spanned"/> — a real spanned ZIP
///      (<c>name.z01..zN</c> + <c>name.zip</c>) or 7z split
///      (<c>name.7z.001..NNN</c>) that requires container-aware
///      readers. Surfaced to the user with a clear "v1.1 예정" card.
///
/// All probing is filesystem-only — no archive open attempted. Cheap
/// enough to call inside the drop classifier.
/// </summary>
internal static class SplitArchiveDetector
{
    /// <summary>
    /// Result of probing a path the user dropped. <see cref="Volumes"/>
    /// is ordered by volume index (1-based); <see cref="EntryPath"/> is
    /// the path the user actually clicked / dropped (which may be any
    /// volume in the set, not necessarily the first).
    /// </summary>
    public readonly record struct DetectionResult(
        SplitKind Kind,
        IReadOnlyList<string> Volumes,
        string EntryPath,
        string Stem);

    /// <summary>
    /// Probe <paramref name="path"/> and its parent directory for split
    /// siblings. Returns <see cref="SplitKind.None"/> when the file
    /// doesn't look like part of a multi-volume set.
    /// </summary>
    public static DetectionResult Probe(string path)
    {
        if (string.IsNullOrEmpty(path) || !File.Exists(path))
        {
            return new DetectionResult(SplitKind.None, Array.Empty<string>(), path, string.Empty);
        }

        // Order matters — RawByteSplit (.001..NNN) check first because it
        // has the most specific suffix shape; Spanned forms come next.
        var raw = TryProbeRawByteSplit(path);
        if (raw.Kind != SplitKind.None) return raw;

        var spanned = TryProbeSpannedZip(path);
        if (spanned.Kind != SplitKind.None) return spanned;

        var seven = TryProbeSeven(path);
        if (seven.Kind != SplitKind.None) return seven;

        return new DetectionResult(SplitKind.None, Array.Empty<string>(), path, string.Empty);
    }

    // ============================================================
    //  Raw byte-split  (name.zip.001, name.zip.002, ...)
    //  Convention: any archive extension followed by .NNN numeric
    //  suffix where NNN >= 001. WinRAR / 7-Zip "split to volumes"
    //  without container-level support produce this layout.
    // ============================================================
    private static readonly Regex RawByteSplitPattern = new(
        @"^(?<stem>.+)\.(?<idx>\d{3,4})$",
        RegexOptions.Compiled | RegexOptions.CultureInvariant,
        TimeSpan.FromSeconds(1));

    private static DetectionResult TryProbeRawByteSplit(string path)
    {
        string name = Path.GetFileName(path);
        var match = RawByteSplitPattern.Match(name);
        if (!match.Success) return Empty(path);

        string stem = match.Groups["stem"].Value;
        // The stem must end with a recognised archive extension —
        // otherwise we'd pick up "report.001" style numbered files
        // that are not archives at all.
        if (!HasArchiveExtension(stem)) return Empty(path);

        string parent = Path.GetDirectoryName(path) ?? string.Empty;
        var volumes = EnumerateSequential(parent, stem, indexWidth: match.Groups["idx"].Length, suffixSeparator: ".");
        if (volumes.Count < 2)
        {
            return Empty(path);
        }
        return new DetectionResult(SplitKind.RawByteSplit, volumes, path, stem);
    }

    // ============================================================
    //  Spanned ZIP  (name.z01, name.z02, ..., name.zip)
    //  APPNOTE.TXT §8 multi-disk archive. Last volume has the EOCD
    //  and uses the bare .zip extension.
    // ============================================================
    private static readonly Regex SpannedZipMidPattern = new(
        @"^(?<stem>.+)\.z(?<idx>\d{2,3})$",
        RegexOptions.Compiled | RegexOptions.CultureInvariant | RegexOptions.IgnoreCase,
        TimeSpan.FromSeconds(1));

    private static DetectionResult TryProbeSpannedZip(string path)
    {
        string name = Path.GetFileName(path);
        string parent = Path.GetDirectoryName(path) ?? string.Empty;
        string? stem = null;

        // Case A: user dropped a mid-volume (.z01..zN).
        var mid = SpannedZipMidPattern.Match(name);
        if (mid.Success)
        {
            stem = mid.Groups["stem"].Value;
        }
        // Case B: user dropped the last volume (.zip) — verify siblings
        // by checking if name.z01 exists alongside.
        else if (name.EndsWith(".zip", StringComparison.OrdinalIgnoreCase))
        {
            string candidateStem = name.Substring(0, name.Length - ".zip".Length);
            string sibling = Path.Combine(parent, candidateStem + ".z01");
            if (File.Exists(sibling))
            {
                stem = candidateStem;
            }
        }
        if (stem is null) return Empty(path);

        var volumes = EnumerateSpannedZip(parent, stem);
        if (volumes.Count < 2) return Empty(path);
        return new DetectionResult(SplitKind.Spanned, volumes, path, stem);
    }

    // ============================================================
    //  7z split  (name.7z.001, name.7z.002, ...)
    //  Convention: 7-Zip's built-in "split into volumes" with the
    //  .7z container. Different from RawByteSplit because the .001
    //  IS the first 7z volume; the container itself is spanning-aware.
    // ============================================================
    private static readonly Regex SevenSplitPattern = new(
        @"^(?<stem>.+\.7z)\.(?<idx>\d{3,4})$",
        RegexOptions.Compiled | RegexOptions.CultureInvariant | RegexOptions.IgnoreCase,
        TimeSpan.FromSeconds(1));

    private static DetectionResult TryProbeSeven(string path)
    {
        string name = Path.GetFileName(path);
        var match = SevenSplitPattern.Match(name);
        if (!match.Success) return Empty(path);

        string stem = match.Groups["stem"].Value; // includes the ".7z"
        string parent = Path.GetDirectoryName(path) ?? string.Empty;
        var volumes = EnumerateSequential(parent, stem, indexWidth: match.Groups["idx"].Length, suffixSeparator: ".");
        if (volumes.Count < 2) return Empty(path);
        return new DetectionResult(SplitKind.Spanned, volumes, path, stem);
    }

    // ------------------------------------------------------------
    //  Helpers
    // ------------------------------------------------------------

    private static List<string> EnumerateSequential(string parent, string stem, int indexWidth, string suffixSeparator)
    {
        var found = new List<string>();
        for (int idx = 1; idx <= 9999; idx++)
        {
            string padded = idx.ToString(CultureInfo.InvariantCulture).PadLeft(indexWidth, '0');
            string candidate = Path.Combine(parent, stem + suffixSeparator + padded);
            if (!File.Exists(candidate))
            {
                if (found.Count == 0) continue; // user-dropped index may start above 1
                break;
            }
            found.Add(candidate);
        }
        return found;
    }

    private static List<string> EnumerateSpannedZip(string parent, string stem)
    {
        var found = new List<string>();
        for (int idx = 1; idx <= 999; idx++)
        {
            string padded = idx.ToString(CultureInfo.InvariantCulture).PadLeft(2, '0');
            string candidate = Path.Combine(parent, stem + ".z" + padded);
            if (!File.Exists(candidate))
            {
                break;
            }
            found.Add(candidate);
        }
        // The bare .zip closes the set when present.
        string final = Path.Combine(parent, stem + ".zip");
        if (File.Exists(final))
        {
            found.Add(final);
        }
        return found;
    }

    private static bool HasArchiveExtension(string nameWithoutNumericSuffix)
    {
        string ext = Path.GetExtension(nameWithoutNumericSuffix);
        if (string.IsNullOrEmpty(ext)) return false;
        return ext.Equals(".zip", StringComparison.OrdinalIgnoreCase)
            || ext.Equals(".7z", StringComparison.OrdinalIgnoreCase)
            || ext.Equals(".rar", StringComparison.OrdinalIgnoreCase)
            || ext.Equals(".tar", StringComparison.OrdinalIgnoreCase)
            || ext.Equals(".gz", StringComparison.OrdinalIgnoreCase)
            || ext.Equals(".bz2", StringComparison.OrdinalIgnoreCase)
            || ext.Equals(".xz", StringComparison.OrdinalIgnoreCase)
            || ext.Equals(".zst", StringComparison.OrdinalIgnoreCase);
    }

    private static DetectionResult Empty(string path)
        => new(SplitKind.None, Array.Empty<string>(), path, string.Empty);
}
