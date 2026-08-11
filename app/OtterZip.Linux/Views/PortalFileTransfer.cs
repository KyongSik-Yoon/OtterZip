// OtterZip for Linux — the freedesktop document-portal file-transfer receiver.
//
// A portal-mediated drag (KDE Plasma especially, and any Wayland→XWayland drop
// routed through the document portal) does NOT put file paths on the drag. It
// puts a single transfer KEY in application/vnd.portal.filetransfer, and the
// real paths are fetched from org.freedesktop.portal.FileTransfer.RetrieveFiles
// over D-Bus. This is why a KDE drop advertises a "File" format — so the window
// highlights — yet carries no readable URI.
//
// Shelling to gdbus keeps this dependency-free: gdbus ships with GLib, which is
// present wherever a portal is. No D-Bus client library is pulled in.

using System;
using System.Collections.Generic;
using System.Diagnostics;

namespace OtterZip.Linux.Views;

internal static class PortalFileTransfer
{
    /// <summary>
    /// Resolve a <c>application/vnd.portal.filetransfer</c> key to the real
    /// local paths via the document portal. Returns empty on any failure
    /// (gdbus absent, portal refused, key already consumed).
    /// </summary>
    public static List<string> Retrieve(string key)
    {
        var files = new List<string>();
        key = Clean(key);
        if (key.Length == 0)
        {
            return files;
        }
        (int code, string stdout, _) = RunGdbus(key);
        if (code == 0)
        {
            ParseGVariantStrings(stdout, files);
        }
        return files;
    }

    /// <summary>
    /// Short human-readable outcome of the portal call, for the on-screen drop
    /// diagnostic: <c>ok:N</c>, <c>empty</c>, <c>gdbus-missing</c>, or
    /// <c>exit&lt;code&gt;:&lt;stderr first line&gt;</c>.
    /// </summary>
    public static string Describe(string key)
    {
        key = Clean(key);
        if (key.Length == 0)
        {
            return "nokey";
        }
        (int code, string stdout, string stderr) = RunGdbus(key);
        if (code == int.MinValue)
        {
            return "gdbus-missing";
        }
        if (code != 0)
        {
            string first = stderr.Split('\n', 2)[0].Trim();
            return "exit" + code.ToString(System.Globalization.CultureInfo.InvariantCulture)
                + (first.Length > 0 ? ":" + Truncate(first, 80) : string.Empty);
        }
        var files = new List<string>();
        ParseGVariantStrings(stdout, files);
        return files.Count > 0 ? "ok:" + files.Count.ToString(System.Globalization.CultureInfo.InvariantCulture) : "empty";
    }

    private static string Clean(string key) => key.Trim().Trim('\0').Trim();

    /// <summary>
    /// Call <c>org.freedesktop.portal.FileTransfer.RetrieveFiles</c> via gdbus.
    /// Returns (exitCode, stdout, stderr); exitCode is <see cref="int.MinValue"/>
    /// when gdbus itself could not be launched.
    /// </summary>
    private static (int code, string stdout, string stderr) RunGdbus(string key)
    {
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "gdbus",
                UseShellExecute = false,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
            };
            foreach (string a in new[]
            {
                "call", "--session",
                "--dest", "org.freedesktop.portal.Documents",
                "--object-path", "/org/freedesktop/portal/documents",
                "--method", "org.freedesktop.portal.FileTransfer.RetrieveFiles",
                key,
                "@a{sv} {}", // empty, explicitly-typed options dict
            })
            {
                psi.ArgumentList.Add(a);
            }
            using var p = Process.Start(psi);
            if (p is null)
            {
                return (int.MinValue, string.Empty, string.Empty);
            }
            string stdout = p.StandardOutput.ReadToEnd();
            string stderr = p.StandardError.ReadToEnd();
            if (!p.WaitForExit(5000))
            {
                try { p.Kill(); } catch (Exception) { /* already gone */ }
                return (int.MinValue, string.Empty, "timeout");
            }
            return (p.ExitCode, stdout, stderr);
        }
        catch (Exception ex) when (ex is System.ComponentModel.Win32Exception or InvalidOperationException)
        {
            return (int.MinValue, string.Empty, ex.Message);
        }
    }

    /// <summary>
    /// Pull the single-quoted strings out of gdbus's GVariant text output.
    /// A single quote inside a path is escaped by gdbus as <c>\'</c>; unescape
    /// it so such paths survive.
    /// </summary>
    private static void ParseGVariantStrings(string text, List<string> into)
    {
        int i = 0;
        while (i < text.Length)
        {
            int start = text.IndexOf('\'', i);
            if (start < 0)
            {
                break;
            }
            int j = start + 1;
            var sb = new System.Text.StringBuilder();
            bool closed = false;
            while (j < text.Length)
            {
                char c = text[j];
                if (c == '\\' && j + 1 < text.Length)
                {
                    sb.Append(text[j + 1]);
                    j += 2;
                    continue;
                }
                if (c == '\'')
                {
                    closed = true;
                    break;
                }
                sb.Append(c);
                j++;
            }
            if (!closed)
            {
                break;
            }
            if (sb.Length > 0)
            {
                into.Add(sb.ToString());
            }
            i = j + 1;
        }
    }

    private static string Truncate(string s, int max) =>
        s.Length <= max ? s : s[..max] + "…";
}
