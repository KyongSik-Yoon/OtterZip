// OtterZip for Linux — freedesktop.org replacements for the Win32 shell
// affordances.
//
// Keeps the WinUI class name and method signatures because the shared
// CompressEngine calls `Win32Helper.MoveToRecycleBin`. The name is a lie on
// this platform, but a truthful rename would fork a file that is otherwise
// identical in both front ends, and the alternative — an interface plus a
// factory for three methods — buys nothing.
//
// Each method maps a Windows shell concept onto its freedesktop equivalent:
//
//   Recycle Bin       → XDG Trash spec ($XDG_DATA_HOME/Trash), with a
//                       `gio trash` fast path so the desktop's own trash
//                       implementation (and its undo) is used when present.
//   Explorer /select  → `xdg-open` on the containing directory. There is no
//                       portable "select this file" verb; file managers that
//                       support it do so through a D-Bus interface, tried
//                       first below.
//   MessageBeep       → `canberra-gtk-play`, the XDG sound-theme player.

using System;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Text;

namespace OtterZip.App.Services;

public static class Win32Helper
{
    /// <summary>
    /// Plays the desktop's "complete" event sound, when
    /// <c>Settings_PlaySoundOnCompress</c> / <c>Settings_PlaySoundOnExtract</c>
    /// is on. Silent no-op when no sound-theme player is installed — a chime
    /// is never worth failing or delaying an archive operation for.
    /// </summary>
    public static void PlayCompletionSound()
    {
        // `complete` is a standard XDG sound-naming-spec event id, present in
        // freedesktop, Adwaita and Breeze sound themes alike.
        _ = TryRun("canberra-gtk-play", "-i", "complete");
    }

    /// <summary>
    /// Opens the user's file manager showing <paramref name="path"/>.
    /// </summary>
    /// <returns>
    /// <c>true</c> when a file manager was launched, <c>false</c> on any
    /// failure (no path, nothing registered to open a directory).
    /// </returns>
    public static bool RevealInExplorer(string path)
    {
        ArgumentException.ThrowIfNullOrEmpty(path);

        // Preferred: the org.freedesktop.FileManager1 interface, which every
        // major file manager (Nautilus, Dolphin, Nemo, Thunar, PCManFM)
        // implements and which actually SELECTS the file rather than just
        // opening its folder — the behaviour `explorer /select,` gives on
        // Windows. Addressed through `gdbus` so this stays dependency-free.
        string uri = new Uri(Path.GetFullPath(path)).AbsoluteUri;
        if (TryRun(
                "gdbus", "call", "--session",
                "--dest", "org.freedesktop.FileManager1",
                "--object-path", "/org/freedesktop/FileManager1",
                "--method", "org.freedesktop.FileManager1.ShowItems",
                "[\"" + uri + "\"]", ""))
        {
            return true;
        }

        // Fallback: open the containing directory. Loses the selection, but
        // every desktop has something registered for `inode/directory`.
        string? dir = Directory.Exists(path) ? path : Path.GetDirectoryName(Path.GetFullPath(path));
        return !string.IsNullOrEmpty(dir) && TryRun("xdg-open", dir);
    }

    /// <summary>
    /// Sends a file or directory to the trash. Returns <c>true</c> on
    /// success.
    /// </summary>
    /// <remarks>
    /// Tries <c>gio trash</c> first so the desktop's own implementation
    /// handles it — that keeps the file visible in the user's Trash UI with
    /// working "restore", and handles the cross-filesystem cases (a file on a
    /// removable volume trashes to <c>.Trash-$uid</c> on that volume, not to
    /// the home directory). Falls back to implementing the XDG Trash spec
    /// directly for minimal systems without glib's CLI tools.
    /// </remarks>
    public static bool MoveToRecycleBin(string path)
    {
        ArgumentException.ThrowIfNullOrEmpty(path);
        if (!File.Exists(path) && !Directory.Exists(path))
        {
            return false;
        }
        if (TryRun("gio", "trash", "--", Path.GetFullPath(path)))
        {
            return true;
        }
        return TrashByHand(Path.GetFullPath(path));
    }

    /// <summary>
    /// Minimal XDG Trash implementation: move the item under
    /// <c>$XDG_DATA_HOME/Trash/files/</c> and drop a matching
    /// <c>.trashinfo</c> next to it in <c>info/</c> recording the original
    /// path and deletion time, which is what makes "restore" possible.
    /// </summary>
    /// <remarks>
    /// Only the home trash is implemented. An item on another filesystem
    /// cannot be moved there with a rename, and copy+delete would silently
    /// turn "send to trash" into a potentially enormous copy — so that case
    /// reports failure and the caller keeps the file, which is the safe way
    /// to be wrong.
    /// </remarks>
    private static bool TrashByHand(string fullPath)
    {
        try
        {
            string dataHome = Environment.GetEnvironmentVariable("XDG_DATA_HOME") is { } xdg
                && Path.IsPathRooted(xdg)
                    ? xdg
                    : Path.Combine(
                        Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                        ".local", "share");
            string trash = Path.Combine(dataHome, "Trash");
            string filesDir = Path.Combine(trash, "files");
            string infoDir = Path.Combine(trash, "info");
            Directory.CreateDirectory(filesDir);
            Directory.CreateDirectory(infoDir);

            string name = Path.GetFileName(fullPath);
            string target = Path.Combine(filesDir, name);
            // The spec requires the name in files/ and info/ to agree and be
            // unique; collisions are common (deleting `build/` twice).
            for (int i = 1; File.Exists(target) || Directory.Exists(target)
                            || File.Exists(target + ".trashinfo"); i++)
            {
                target = Path.Combine(
                    filesDir,
                    string.Create(CultureInfo.InvariantCulture, $"{name}.{i}"));
            }

            // Write the info file BEFORE the move. If we crash between the
            // two, the spec's own guidance is that an info file with no
            // matching item is the recoverable direction — an orphaned item
            // with no info file can never be restored.
            var info = new StringBuilder()
                .Append("[Trash Info]\n")
                .Append("Path=").Append(Uri.EscapeDataString(fullPath).Replace("%2F", "/", StringComparison.Ordinal)).Append('\n')
                .Append("DeletionDate=")
                .Append(DateTime.Now.ToString("yyyy-MM-ddTHH:mm:ss", CultureInfo.InvariantCulture))
                .Append('\n');
            File.WriteAllText(Path.Combine(infoDir, Path.GetFileName(target) + ".trashinfo"), info.ToString());

            if (Directory.Exists(fullPath))
            {
                Directory.Move(fullPath, target);
            }
            else
            {
                File.Move(fullPath, target);
            }
            return true;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or NotSupportedException)
        {
            // Cross-device move, permissions, or no writable home. Report
            // failure and leave the file where it is.
            return false;
        }
    }

    /// <summary>
    /// Run a helper program and report whether it exited successfully.
    /// Everything here is optional desktop plumbing, so a missing binary is
    /// a <c>false</c>, never an exception.
    /// </summary>
    private static bool TryRun(string file, params string[] args)
    {
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = file,
                UseShellExecute = false,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
            };
            foreach (string a in args)
            {
                psi.ArgumentList.Add(a);
            }
            using Process? p = Process.Start(psi);
            if (p is null)
            {
                return false;
            }
            // Bounded wait: a misbehaving helper must not hang the caller,
            // which for PlayCompletionSound is the UI thread's continuation.
            if (!p.WaitForExit(5000))
            {
                return false;
            }
            return p.ExitCode == 0;
        }
        catch (Exception ex) when (ex is System.ComponentModel.Win32Exception or InvalidOperationException or PlatformNotSupportedException)
        {
            return false;
        }
    }
}
