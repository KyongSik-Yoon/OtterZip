using System;
using System.Collections.Generic;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
using OtterZip.Interop;

namespace OtterZip.App.Services;

/// <summary>
/// Headless compress execution path — extracts the plan-building,
/// FFI invocation, verification, recycle-bin handling, and cancel-time
/// partial-archive cleanup out of <c>MainWindow.xaml.cs</c> so the
/// Bandizip-style <c>ProgressDialog</c> can run identical compress work
/// without depending on <c>MainWindow</c>.
///
/// <para>
/// All members static. The two entry points are <see cref="BuildPlan"/>
/// (resolve destination + format + method + level from settings + an
/// optional explicit format tag from quick verbs) and
/// <see cref="RunAsync"/> (drive the FFI call + verify + recycle).
/// </para>
/// </summary>
internal static class CompressEngine
{
    /// <summary>
    /// Final compress plan handed to <see cref="ArchiveBuilder"/>.
    /// Decoupled from any UI state so headless flows can build one too.
    /// </summary>
    public sealed record CompressPlan(
        string Destination,
        ArchiveFormat Format,
        CompressionMethod Method,
        byte Level);

    /// <summary>
    /// Build a <see cref="CompressPlan"/> from a multi-selection.
    /// <paramref name="formatOverride"/> wins when set (quick verbs like
    /// <c>compress-zip</c> / <c>compress-7z</c>); otherwise the caller's
    /// dropdown selection (or <c>Settings_DefaultFormat</c>) is used.
    /// </summary>
    public static CompressPlan BuildPlan(
        IReadOnlyList<string> sources,
        string formatTag,
        string? formatOverride = null)
    {
        string effectiveFormat = string.IsNullOrEmpty(formatOverride)
            ? formatTag
            : formatOverride;

        bool useParent = SettingsService.Get<bool>("Settings_UseParentFolderName", true);
        string stem = OutputNamer.DeriveStem(sources, useParent);

        // PR-7B: filename template overrides the default stem when set.
        string template = SettingsService.Get<string>("Settings_FilenameTemplate", "");
        if (!string.IsNullOrWhiteSpace(template) && sources is { Count: > 0 })
        {
            string firstParent = Path.GetDirectoryName(sources[0]) ?? "";
            stem = OutputNamer.ApplyFilenameTemplate(template, stem, firstParent, sources.Count);
        }

        int methodIndex = Math.Clamp(
            SettingsService.Get<int>("Settings_DefaultMethodIndex", 2), 0, 3);
        var (fmt, method, ext) = MapFormatAndMethod(effectiveFormat, methodIndex);
        byte level = MapMethodIndexToLevel(methodIndex);

        string saveLoc = SettingsService.Get<string>("Settings_SaveLocation", "same");
        string customDir = SettingsService.Get<string>("Settings_SaveLocationPath", "");
        string destination = OutputNamer.Compose(sources, stem, ext, saveLoc, customDir);

        return new CompressPlan(destination, fmt, method, level);
    }

    /// <summary>
    /// Drive the actual archive build. Mirrors the previous
    /// <c>MainWindow.RunCompressAsync</c>: directory-single-source goes
    /// through <see cref="ArchiveBuilder.CreateFromDirectoryAsync"/>
    /// (ABI v7 progress hook); anything else loops via
    /// <see cref="CompressMixedSources"/> on a thread-pool task.
    /// </summary>
    public static Task<ArchiveBuildReport> RunAsync(
        CompressPlan plan,
        IReadOnlyList<string> sources,
        string? password,
        IProgress<ProgressUpdate>? progress,
        CancellationToken ct)
    {
        bool excludeMeta = SettingsService.Get<bool>("Settings_ExcludeSystemMetadata", true);

        if (sources.Count == 1 && Directory.Exists(sources[0]))
        {
            return ArchiveBuilder.CreateFromDirectoryAsync(
                plan.Destination, sources[0], plan.Format, plan.Method,
                plan.Level, progress: progress,
                excludeSystemMetadata: excludeMeta,
                password: password,
                cancellationToken: ct);
        }
        return Task.Run(() => CompressMixedSources(plan, sources, excludeMeta, password, ct), ct);
    }

    /// <summary>
    /// Multi-source / mixed file+folder fallback. No mid-write progress
    /// hook in the per-entry <see cref="ArchiveBuilder"/> API yet — the
    /// card stays indeterminate on this path until that ABI lands.
    /// </summary>
    private static ArchiveBuildReport CompressMixedSources(
        CompressPlan plan,
        IReadOnlyList<string> sources,
        bool excludeSystemMetadata,
        string? password,
        CancellationToken ct)
    {
        using var builder = ArchiveBuilder.Create(
            plan.Destination, plan.Format, plan.Method, plan.Level,
            solid: false,
            excludeSystemMetadata: excludeSystemMetadata,
            password: password);
        foreach (string src in sources)
        {
            ct.ThrowIfCancellationRequested();
            if (Directory.Exists(src))
            {
                builder.AddDirectory(src, Path.GetFileName(src));
            }
            else
            {
                builder.AddFile(src, Path.GetFileName(src));
            }
        }
        builder.Commit();
        ulong size = 0;
        try { size = (ulong)new FileInfo(plan.Destination).Length; }
        catch (IOException) { }
        return new ArchiveBuildReport { BytesWritten = size };
    }

    /// <summary>
    /// Honour <c>Settings_VerifyAfterCompress</c>: re-open the
    /// just-written archive and CRC-verify every entry. Throws on
    /// corruption; caller surfaces as a normal error toast.
    /// </summary>
    public static async Task MaybeVerifyAsync(string archivePath, CancellationToken ct)
    {
        if (!SettingsService.Get<bool>("Settings_VerifyAfterCompress", false))
        {
            return;
        }
        using var archive = Archive.Open(archivePath);
        var report = await archive.TestAsync(ct).ConfigureAwait(false);
        if (!report.IsHealthy)
        {
            throw new InvalidDataException(
                $"Verification failed: {report.EntriesCorrupted}/{report.EntriesTested} entries corrupted");
        }
    }

    /// <summary>
    /// Honour <c>Settings_DeleteSourceAfterCompress</c>: move every
    /// surviving source to the Recycle Bin. Runs only after success +
    /// optional verify, so a failed compress doesn't destroy the user's
    /// input.
    /// </summary>
    public static void MaybeRecycleSources(IReadOnlyList<string> sources)
    {
        if (!SettingsService.Get<bool>("Settings_DeleteSourceAfterCompress", false))
        {
            return;
        }
        foreach (var src in sources)
        {
            _ = Win32Helper.MoveToRecycleBin(src);
        }
    }

    /// <summary>
    /// Best-effort cleanup of a partially-written archive on cancel.
    /// Swallows IO + permission errors — the cancel path runs as a
    /// finalizer and must never crash on file locks.
    /// </summary>
    public static void TryDeletePartialArchive(string path)
    {
        try
        {
            if (File.Exists(path))
            {
                File.Delete(path);
            }
        }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
    }

    /// <summary>
    /// Format-tag → (<see cref="ArchiveFormat"/>, native compression
    /// method, file extension) triple. Drives the ABI selection on the
    /// native ArchiveBuilder.
    ///
    /// <para>MethodIndex 0 (Store) forces the Stored/Copy method
    /// regardless of format; other indices pick the format's natural
    /// compression method and the level encodes Fast / Normal / Best.</para>
    /// </summary>
    public static (ArchiveFormat fmt, CompressionMethod method, string ext) MapFormatAndMethod(
        string formatTag, int methodIndex)
    {
        bool store = methodIndex == 0;
        return formatTag switch
        {
            "7z" => (ArchiveFormat.SevenZ,
                     store ? CompressionMethod.Store : CompressionMethod.Lzma2,
                     ".7z"),
            "tar" => (ArchiveFormat.Tar, CompressionMethod.Store, ".tar"),
            "tar.gz" => (ArchiveFormat.TarGz,
                         store ? CompressionMethod.Store : CompressionMethod.Deflate,
                         ".tar.gz"),
            "tar.bz2" => (ArchiveFormat.TarBz2,
                          store ? CompressionMethod.Store : CompressionMethod.Bzip2,
                          ".tar.bz2"),
            "tar.xz" => (ArchiveFormat.TarXz,
                         store ? CompressionMethod.Store : CompressionMethod.Lzma2,
                         ".tar.xz"),
            "xz" => (ArchiveFormat.Xz,
                     store ? CompressionMethod.Store : CompressionMethod.Lzma2,
                     ".xz"),
            "zipx" => (ArchiveFormat.Zipx,
                       store ? CompressionMethod.Store : CompressionMethod.Bzip2,
                       ".zipx"),
            _ => (ArchiveFormat.Zip,
                  store ? CompressionMethod.Store : CompressionMethod.Deflate,
                  ".zip"),
        };
    }

    /// <summary>
    /// 4-step UI MethodIndex → 1..9 backend level. Store(0) is
    /// irrelevant when method is Stored. idx=2 ("Normal", the default)
    /// maps to level 3 — same as Bandizip's "Fast Drag and Drop" preset
    /// which is the wall-clock the user measures against.
    /// </summary>
    public static byte MapMethodIndexToLevel(int methodIndex) => methodIndex switch
    {
        0 => 1,
        1 => 1,
        2 => 3,
        3 => 9,
        _ => 3,
    };
}
