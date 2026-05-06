using System;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using Microsoft.VisualBasic.FileIO;

namespace SpanZIP.App.Services;

/// <summary>
/// Thin façade for the three Win32 conveniences SpanZIP's Phase 6+ settings
/// surface: completion sound, "Reveal in Explorer", and "send to Recycle Bin".
///
/// All three intentionally use managed BCL APIs (System.Media + Process +
/// Microsoft.VisualBasic.FileIO) instead of raw Win32 P/Invoke. The
/// <see cref="SHFileOperation"/> route is feasible but its struct marshaling
/// (double-null-terminator path lists, AOT trim of <c>SHFILEOPSTRUCT</c>) is
/// brittle in Native AOT. The VB.NET path is trim-safe and battle-tested.
/// </summary>
public static class Win32Helper
{
    // user32!MessageBeep — minimal AOT-safe sound API. WPF's
    // System.Media.SystemSounds isn't available under WinUI 3.
    // Plain DllImport (not LibraryImport) keeps this class non-partial
    // and avoids the AllowUnsafeBlocks requirement that the source
    // generator would impose. Same pattern as MainWindow.GetDpiForWindow.
    private const uint MB_ICONASTERISK = 0x00000040;

    [DllImport("user32.dll", ExactSpelling = true, SetLastError = false)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool MessageBeep(uint uType);

    /// <summary>
    /// Plays the system "Asterisk" sound. Used as the completion chime when
    /// <c>Settings_PlaySoundOnCompress</c> / <c>Settings_PlaySoundOnExtract</c>
    /// is on. Silent no-op when the user has disabled system sounds in
    /// Windows (the OS handles the suppression internally).
    /// </summary>
    public static void PlayCompletionSound()
    {
        try
        {
            // Return value indicates "queued successfully", but a false
            // result on a system without a sound card is benign — we
            // never abort an archive op because of a chime.
            _ = MessageBeep(MB_ICONASTERISK);
        }
        catch (Exception)
        {
            // user32 is a system DLL; Exception path is purely defensive
            // (e.g. some constrained sandbox).
        }
    }

    /// <summary>
    /// Opens File Explorer with <paramref name="path"/> selected (the
    /// "Show in Explorer" affordance). Equivalent to
    /// <c>explorer.exe /select,"path"</c>.
    /// </summary>
    /// <returns><c>true</c> when Explorer was launched, <c>false</c> on
    /// any failure (path missing, Explorer unavailable, very long path).</returns>
    public static bool RevealInExplorer(string path)
    {
        ArgumentException.ThrowIfNullOrEmpty(path);
        try
        {
            // Process.Start parses the argument string itself — explorer
            // requires the literal "/select," form (no space). UseShellExecute
            // false because we want to invoke explorer directly with arguments,
            // not let the shell pick a default verb.
            var psi = new ProcessStartInfo
            {
                FileName = "explorer.exe",
                Arguments = $"/select,\"{path}\"",
                UseShellExecute = false,
            };
            using var p = Process.Start(psi);
            return p is not null;
        }
        catch (Exception)
        {
            return false;
        }
    }

    /// <summary>
    /// Sends a single file or empty directory to the Recycle Bin. Returns
    /// <c>true</c> on success. The Recycle Bin is bypassed automatically
    /// when the file is on a volume without one (e.g. some network shares),
    /// in which case the file is permanently deleted — the OS standard
    /// behavior, matched here.
    /// </summary>
    /// <remarks>
    /// We use <c>Microsoft.VisualBasic.FileIO.FileSystem.DeleteFile</c>
    /// rather than <c>SHFileOperation</c> P/Invoke because:
    ///   * it's trim-safe under Native AOT (no struct marshaling),
    ///   * the per-path call path avoids the historic double-null-terminator
    ///     bug surface of multi-file SHFILEOPSTRUCT.
    /// </remarks>
    public static bool MoveToRecycleBin(string path)
    {
        ArgumentException.ThrowIfNullOrEmpty(path);
        try
        {
            if (Directory.Exists(path))
            {
                FileSystem.DeleteDirectory(
                    path,
                    UIOption.OnlyErrorDialogs,
                    RecycleOption.SendToRecycleBin);
            }
            else if (File.Exists(path))
            {
                FileSystem.DeleteFile(
                    path,
                    UIOption.OnlyErrorDialogs,
                    RecycleOption.SendToRecycleBin);
            }
            else
            {
                return false;
            }
            return true;
        }
        catch (Exception)
        {
            return false;
        }
    }
}
