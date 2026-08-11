// OtterZip for Linux — turning a dropped payload into local file paths.
//
// A single drag drops different things on different desktops, and Avalonia's
// convenience TryGetFiles() only decodes the standard text/uri-list. On KDE
// Plasma a drop advertises a "File" format (so the window highlights) but hands
// the actual URIs over application/x-kde4-urilist — or, under a portal, only
// application/vnd.portal.filetransfer — so TryGetFiles() comes back empty and
// the drop appears to do nothing. This reads the desktop-specific formats too.

using System;
using System.Collections.Generic;
using System.Text;
using Avalonia.Input;
using Avalonia.Platform.Storage;

namespace OtterZip.Linux.Views;

/// <summary>
/// Extracts local filesystem paths from a dropped <see cref="IDataTransfer"/>,
/// coping with the several ways a Linux desktop hands files over a drag.
/// Shared by the main (compress) window and the archive contents (append)
/// window so both accept exactly the same drops.
/// </summary>
internal static class DropData
{
    // uri-list-shaped formats, in preference order. KDE sends its own
    // application/x-kde4-urilist rather than the standard text/uri-list;
    // GNOME's clipboard uses x-special/gnome-copied-files. Each is a set of
    // file:// URIs (gnome-copied-files prefixes a "copy"/"cut" action line,
    // which the parser skips as a non-URI line).
    private static readonly string[] UriListFormats =
    [
        "text/uri-list",
        "application/x-kde4-urilist",
        "x-special/gnome-copied-files",
        "x-special/nautilus-clipboard",
    ];

    /// <summary>The document-portal transfer-key format (KDE / Wayland drops).</summary>
    private const string PortalFormat = "application/vnd.portal.filetransfer";

    /// <summary>Local paths carried by the drop, in the order they appear.</summary>
    public static List<string> LocalPaths(IDataTransfer data)
    {
        var paths = new List<string>();

        // 1) File items — the common path, and all that is needed on Windows,
        //    macOS and a GNOME/X11 drop.
        IStorageItem[]? files = data.TryGetFiles();
        if (files is not null)
        {
            foreach (IStorageItem item in files)
            {
                string? local = LocalPathOf(item);
                if (local is not null)
                {
                    paths.Add(local);
                }
            }
        }

        // Only formats the drop actually advertises are worth reading — asking
        // the X server for one the source does not offer just burns the wait.
        HashSet<string> offered = OfferedFormats(data);

        // 2) Read the selection ourselves for a uri-list format. Avalonia
        //    12.1's X11 raw read is unreliable (it returns the same buffer for
        //    every format), so go straight to the X server for the real bytes.
        if (paths.Count == 0)
        {
            AppendUriListsFromX11(paths, offered);
        }

        // 3) A portal-mediated drop (KDE Plasma, and Wayland→XWayland drags
        //    routed through the document portal) carries only a transfer key
        //    in application/vnd.portal.filetransfer; the real paths come from
        //    the FileTransfer portal over D-Bus. Read the key straight from X11
        //    for the same reason as (2), then resolve it through the portal.
        if (paths.Count == 0 && offered.Contains(PortalFormat))
        {
            string? key = X11Selection.Read(PortalFormat);
            if (!string.IsNullOrEmpty(key))
            {
                paths.AddRange(PortalFileTransfer.Retrieve(key));
            }
        }

        // 4) Last resort: Avalonia's plain-text view of the drop.
        if (paths.Count == 0)
        {
            string? text = data.TryGetText();
            if (!string.IsNullOrEmpty(text))
            {
                AddUriListPaths(text, paths);
            }
        }
        return paths;
    }

    /// <summary>
    /// Per-format diagnostic for the "drop produced nothing" line: each
    /// format identifier with what its raw value came back as — <c>null</c>,
    /// <c>b&lt;n&gt;</c> for n bytes, <c>s&lt;n&gt;</c> for an n-char string, or
    /// <c>err</c>. This says whether the payload was empty or simply in a shape
    /// we do not decode, which is the difference between "KDE left it blank" and
    /// "we missed a format".
    /// </summary>
    public static string Diagnose(IDataTransfer data)
    {
        var parts = new List<string>();
        foreach (IDataTransferItem item in data.Items)
        {
            foreach (DataFormat format in item.Formats)
            {
                string info;
                try
                {
                    object? raw = item.TryGetRaw(format);
                    info = raw switch
                    {
                        null => "null",
                        byte[] b => "b" + b.Length.ToString(System.Globalization.CultureInfo.InvariantCulture)
                            + "[" + Preview(b) + "]",
                        string s => "s" + s.Length.ToString(System.Globalization.CultureInfo.InvariantCulture)
                            + "[" + Preview(Encoding.UTF8.GetBytes(s)) + "]",
                        _ => raw.GetType().Name,
                    };
                }
                catch (Exception)
                {
                    info = "err";
                }
                parts.Add(format.Identifier + "=" + info);
            }
        }
        // What our own X11 read recovers — the values Avalonia's TryGetRaw
        // could not — and, if a portal key comes through, the portal's verdict
        // on it. This is the decisive line: it distinguishes "we now read the
        // key but the portal rejected it" from "the X read itself came up null".
        string? kde = X11Selection.Read("application/x-kde4-urilist");
        parts.Add("x11.kde4=" + Describe(kde));
        string? key = X11Selection.Read("application/vnd.portal.filetransfer");
        parts.Add("x11.portal=" + Describe(key));
        if (!string.IsNullOrEmpty(key))
        {
            parts.Add("portal→" + PortalFileTransfer.Describe(key));
        }
        return parts.Count > 0 ? string.Join(", ", parts) : "(none)";
    }

    private static string Describe(string? value) =>
        value is null
            ? "null"
            : "s" + value.Length.ToString(System.Globalization.CultureInfo.InvariantCulture)
                + "[" + Preview(Encoding.UTF8.GetBytes(value)) + "]";

    /// <summary>
    /// Up to 24 bytes of a raw value rendered as printable ASCII (other bytes
    /// shown as <c>.</c>), so the diagnostic can reveal whether formats carry
    /// distinct content or the same buffer, and whether a "key" looks like one.
    /// </summary>
    private static string Preview(byte[] bytes)
    {
        int n = Math.Min(bytes.Length, 24);
        var sb = new StringBuilder(n);
        for (int i = 0; i < n; i++)
        {
            char c = (char)bytes[i];
            sb.Append(c is >= (char)32 and < (char)127 ? c : '.');
        }
        return sb.ToString();
    }

    /// <summary>
    /// Try each offered uri-list-shaped format by reading the X11 selection
    /// directly, stopping at the first that yields paths.
    /// </summary>
    private static void AppendUriListsFromX11(List<string> paths, HashSet<string> offered)
    {
        foreach (string format in UriListFormats)
        {
            if (!offered.Contains(format))
            {
                continue;
            }
            string? value = X11Selection.Read(format);
            if (!string.IsNullOrEmpty(value))
            {
                AddUriListPaths(value, paths);
            }
            if (paths.Count > 0)
            {
                return;
            }
        }
    }

    /// <summary>Every format identifier the drop advertises, transfer- and item-level.</summary>
    private static HashSet<string> OfferedFormats(IDataTransfer data)
    {
        var set = new HashSet<string>(StringComparer.Ordinal);
        foreach (DataFormat format in data.Formats)
        {
            set.Add(format.Identifier);
        }
        foreach (IDataTransferItem item in data.Items)
        {
            foreach (DataFormat format in item.Formats)
            {
                set.Add(format.Identifier);
            }
        }
        return set;
    }

    private static string? LocalPathOf(IStorageItem item)
    {
        string? local = item.TryGetLocalPath();
        if (!string.IsNullOrEmpty(local))
        {
            return local;
        }
        // A storage item built straight from a file:// URI (the external-drop
        // case) can resolve no BCL path yet still carry the URI on Path.
        Uri? p = item.Path;
        return p is { IsAbsoluteUri: true, IsFile: true } ? p.LocalPath : null;
    }

    private static void AddUriListPaths(string text, List<string> into)
    {
        foreach (string raw in text.Split('\n'))
        {
            // Trim CR and any trailing NUL (KDE null-terminates the buffer).
            string s = raw.Trim().Trim('\0').Trim();
            if (s.Length == 0 || s[0] == '#') // uri-list comments, and the
            {                                  // "copy"/"cut" action line of
                continue;                      // x-special/gnome-copied-files
            }
            if (Uri.TryCreate(s, UriKind.Absolute, out Uri? uri) && uri.IsFile)
            {
                into.Add(uri.LocalPath);
            }
            else if (s[0] == '/')
            {
                into.Add(s);
            }
        }
    }
}
