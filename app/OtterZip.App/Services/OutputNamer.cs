using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;

namespace OtterZip.App.Services;

/// <summary>
/// Archive output-path derivation extracted from MainWindow.PlanCompress
/// so the Bandizip-style ProgressDialog flow can call into the same
/// stem-derivation + collision-safe naming logic without depending on
/// MainWindow internals.
///
/// Three responsibilities:
///   1. Derive a base stem from a multi-selection (single file → its
///      basename, folder → its name, N items → parent-folder name when
///      Settings_UseParentFolderName is on).
///   2. Resolve a save-target directory from settings + the source
///      parent, falling back to Desktop when the source parent is not
///      writable (drive root with admin lock, read-only share, etc.).
///   3. Append `(1)`, `(2)`, ... to the candidate filename until it
///      doesn't clash with an existing file, preserving compound
///      .tar.gz / .tar.bz2 / .tar.xz extensions as a unit.
///
/// All members static — this is a pure namer, not a stateful service.
/// </summary>
internal static class OutputNamer
{
    /// <summary>
    /// Bandizip-style stem: single source uses its own basename; multi
    /// selection uses the parent-folder name when
    /// Settings_UseParentFolderName is true (default), otherwise the
    /// first source's basename. Falls back to "archive" when both yield
    /// whitespace.
    /// </summary>
    public static string DeriveStem(
        IReadOnlyList<string> sources,
        bool useParentFolderName)
    {
        if (sources is null || sources.Count == 0)
        {
            return "archive";
        }
        string first = sources[0];
        string parentDir = Path.GetDirectoryName(first) ?? "";
        string stem = sources.Count == 1
            ? SourceStem(first)
            : useParentFolderName
                ? Path.GetFileName(parentDir.TrimEnd(
                      Path.DirectorySeparatorChar,
                      Path.AltDirectorySeparatorChar))
                : SourceStem(first);
        return string.IsNullOrWhiteSpace(stem) ? "archive" : stem;
    }

    /// <summary>
    /// Folder sources keep their full directory name (a dotted folder
    /// like "Maru.App_1.0.1.0_x64_Test" must NOT be treated as having a
    /// ".0_x64_Test" extension). File sources strip the extension.
    /// </summary>
    public static string SourceStem(string source)
    {
        if (Directory.Exists(source))
        {
            string trimmed = source.TrimEnd(
                Path.DirectorySeparatorChar,
                Path.AltDirectorySeparatorChar);
            return Path.GetFileName(trimmed);
        }
        return Path.GetFileNameWithoutExtension(source);
    }

    /// <summary>
    /// Filename-template substitution (PR-7B). Tokens: `{name}`,
    /// `{date}`, `{time}`, `{count}`, `{parent}`. Result is sanitised
    /// against Windows-illegal filename chars so the user can't smuggle
    /// a colon or asterisk into the path. Empty template returns the
    /// original stem unchanged.
    /// </summary>
    public static string ApplyFilenameTemplate(
        string template,
        string name,
        string parentDir,
        int count)
    {
        if (string.IsNullOrWhiteSpace(template))
        {
            return name;
        }
        var now = DateTime.Now;
        string parent = string.IsNullOrEmpty(parentDir)
            ? ""
            : Path.GetFileName(parentDir);
        string result = template
            .Replace("{name}", name, StringComparison.Ordinal)
            .Replace("{date}", now.ToString("yyyy-MM-dd", CultureInfo.InvariantCulture), StringComparison.Ordinal)
            .Replace("{time}", now.ToString("HHmm", CultureInfo.InvariantCulture), StringComparison.Ordinal)
            .Replace("{count}", count.ToString(CultureInfo.InvariantCulture), StringComparison.Ordinal)
            .Replace("{parent}", parent, StringComparison.Ordinal);

        foreach (char invalid in Path.GetInvalidFileNameChars())
        {
            result = result.Replace(invalid.ToString(), "", StringComparison.Ordinal);
        }
        return string.IsNullOrWhiteSpace(result) ? name : result;
    }

    /// <summary>
    /// Resolve the parent directory we'll write the archive into:
    /// honours Settings_SaveLocation = "custom" + Settings_SaveLocationPath
    /// when both are set, otherwise places the archive next to the first
    /// source. Falls back to Desktop when the chosen parent is missing
    /// or not writable (admin-locked drive root, read-only share, etc.).
    /// </summary>
    public static string ResolveParentDir(
        IReadOnlyList<string> sources,
        string saveLocation,
        string customDir)
    {
        string defaultParent = sources is { Count: > 0 }
            ? Path.GetDirectoryName(sources[0]) ?? Directory.GetCurrentDirectory()
            : Directory.GetCurrentDirectory();

        string chosen = defaultParent;
        if (string.Equals(saveLocation, "custom", StringComparison.Ordinal)
            && !string.IsNullOrWhiteSpace(customDir)
            && Directory.Exists(customDir))
        {
            chosen = customDir;
        }

        if (!CanWriteProbe(chosen))
        {
            string desktop = Environment.GetFolderPath(
                Environment.SpecialFolder.DesktopDirectory);
            if (!string.IsNullOrEmpty(desktop) && CanWriteProbe(desktop))
            {
                return desktop;
            }
        }
        return chosen;
    }

    // ------------------------------------------------------------------
    //  In-process output reservation (pre-1.0 review C2)
    //
    //  All plans compute their destination BEFORE any job starts writing,
    //  and the default concurrency is 2 — so two same-stem plans (e.g.
    //  "report.docx" + "report.pdf" compressed individually) both probed
    //  File.Exists("report.zip") = false and two native writers opened the
    //  SAME file. Real submit points therefore reserve their name in this
    //  registry; preview paths keep using the un-reserving EnsureUnique.
    //  Cross-process races remain out of scope (as with every archiver).
    // ------------------------------------------------------------------
    private static readonly System.Threading.Lock s_reserveLock = new();
    private static readonly HashSet<string> s_reserved = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>
    /// Collision-safe name that is also atomically registered against
    /// other in-flight jobs in this process. Use at REAL submit points
    /// only (a preview must not burn names). Pair with <see cref="Release"/>
    /// when the output is cleaned up after cancel/failure.
    /// </summary>
    public static string ReserveUnique(string path)
    {
        lock (s_reserveLock)
        {
            string candidate = UniquifyFile(path,
                p => File.Exists(p) || s_reserved.Contains(p));
            s_reserved.Add(candidate);
            return candidate;
        }
    }

    /// <summary>Directory flavour of <see cref="ReserveUnique"/> — the
    /// numeric suffix appends after the full folder name ("app.v2 (1)").</summary>
    public static string ReserveUniqueDirectory(string path)
    {
        lock (s_reserveLock)
        {
            string candidate = UniquifyDirectory(path,
                p => Directory.Exists(p) || File.Exists(p) || s_reserved.Contains(p));
            s_reserved.Add(candidate);
            return candidate;
        }
    }

    /// <summary>Free a reservation whose output was removed again
    /// (cancel / failure cleanup). Safe to call for unreserved paths.</summary>
    public static void Release(string path)
    {
        lock (s_reserveLock)
        {
            s_reserved.Remove(path);
        }
    }

    /// <summary>
    /// Bandizip / Windows Explorer "Copy" naming: foo.zip exists →
    /// "foo (1).zip"; if that exists too, "foo (2).zip"; up to 9999.
    /// Compound .tar.gz / .tar.bz2 / .tar.xz extensions stay intact so
    /// the numeric suffix lands at the right position. Beyond 9999
    /// duplicates we stamp with a timestamp rather than blowing the
    /// loop (pathological — millions-of-clones territory).
    ///
    /// NOTE: probes the filesystem only — no cross-job reservation. Fine
    /// for previews; real submit points go through <see cref="ReserveUnique"/>.
    /// </summary>
    public static string EnsureUnique(string path)
        => UniquifyFile(path, File.Exists);

    private static string UniquifyFile(string path, Func<string, bool> isTaken)
    {
        if (!isTaken(path)) return path;
        string dir = Path.GetDirectoryName(path) ?? Directory.GetCurrentDirectory();
        string nameOnly = Path.GetFileNameWithoutExtension(path);
        string ext = Path.GetExtension(path);
        if (nameOnly.EndsWith(".tar", StringComparison.OrdinalIgnoreCase)
            && (string.Equals(ext, ".gz", StringComparison.OrdinalIgnoreCase)
                || string.Equals(ext, ".bz2", StringComparison.OrdinalIgnoreCase)
                || string.Equals(ext, ".xz", StringComparison.OrdinalIgnoreCase)))
        {
            nameOnly = Path.GetFileNameWithoutExtension(nameOnly);
            ext = ".tar" + ext;
        }
        for (int i = 1; i < 10000; i++)
        {
            string candidate = Path.Combine(dir, string.Format(
                CultureInfo.InvariantCulture, "{0} ({1}){2}", nameOnly, i, ext));
            if (!isTaken(candidate)) return candidate;
        }
        return Path.Combine(dir, string.Format(CultureInfo.InvariantCulture,
            "{0} ({1:yyyyMMddHHmmss}){2}", nameOnly, DateTime.Now, ext));
    }

    private static string UniquifyDirectory(string path, Func<string, bool> isTaken)
    {
        if (!isTaken(path)) return path;
        string parent = Path.GetDirectoryName(path) ?? Directory.GetCurrentDirectory();
        string name = Path.GetFileName(path.TrimEnd(
            Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar));
        for (int i = 1; i < 10000; i++)
        {
            string candidate = Path.Combine(parent, string.Format(
                CultureInfo.InvariantCulture, "{0} ({1})", name, i));
            if (!isTaken(candidate)) return candidate;
        }
        return Path.Combine(parent, string.Format(CultureInfo.InvariantCulture,
            "{0} ({1:yyyyMMddHHmmss})", name, DateTime.Now));
    }

    /// <summary>
    /// Convenience composer used by ProgressDialog and (eventually)
    /// PlanCompress: stem + extension → collision-safe absolute path
    /// rooted under the resolved parent dir.
    /// </summary>
    public static string Compose(
        IReadOnlyList<string> sources,
        string stem,
        string ext,
        string saveLocation,
        string customDir)
    {
        string parentDir = ResolveParentDir(sources, saveLocation, customDir);
        return EnsureUnique(Path.Combine(parentDir, stem + ext));
    }

    /// <summary>
    /// Best-effort write probe: tries to create + delete a transient
    /// `.otterzip-write-probe-<guid>` in the target. Doesn't actually
    /// reserve a slot — the probe is informational. Caller still has to
    /// handle IOException on the real File.Create.
    /// </summary>
    private static bool CanWriteProbe(string dir)
    {
        if (string.IsNullOrWhiteSpace(dir) || !Directory.Exists(dir))
        {
            return false;
        }
        string probe = Path.Combine(dir,
            ".otterzip-write-probe-" + Guid.NewGuid().ToString("N"));
        try
        {
            using (File.Create(probe)) { }
            File.Delete(probe);
            return true;
        }
        catch
        {
            return false;
        }
    }
}
