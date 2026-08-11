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

        // 2) A raw uri-list under a desktop-specific format.
        if (paths.Count == 0)
        {
            foreach (string uriList in RawValues(data, UriListFormats))
            {
                AddUriListPaths(uriList, paths);
            }
        }

        // 3) A portal-mediated drop (KDE Plasma, and Wayland→XWayland drags
        //    routed through the document portal) carries only a transfer key
        //    in application/vnd.portal.filetransfer; the real paths come from
        //    the FileTransfer portal over D-Bus. This is the KDE path — its
        //    x-kde4-urilist is advertised but left empty.
        if (paths.Count == 0)
        {
            foreach (string key in RawValues(data, "application/vnd.portal.filetransfer"))
            {
                paths.AddRange(PortalFileTransfer.Retrieve(key));
            }
        }

        // 4) Last resort: a plain-text payload of URIs or bare paths.
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
        // If a portal key is present, show the outcome of actually calling the
        // FileTransfer portal with it — the decisive fact when file items and
        // raw uri-lists both came up empty.
        foreach (string key in RawValues(data, "application/vnd.portal.filetransfer"))
        {
            parts.Add("portal→" + PortalFileTransfer.Describe(key));
            break;
        }
        return parts.Count > 0 ? string.Join(", ", parts) : "(none)";
    }

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

    /// <summary>
    /// Decoded raw value (bytes as UTF-8, or a string) of every drop item
    /// whose format identifier is one of <paramref name="identifiers"/>. The
    /// lookup uses the actual <see cref="DataFormat"/> object from the drop, so
    /// its kind (platform vs application) matches and
    /// <see cref="IDataTransferItem.TryGetRaw"/> does not miss.
    /// </summary>
    private static IEnumerable<string> RawValues(IDataTransfer data, params string[] identifiers)
    {
        foreach (IDataTransferItem item in data.Items)
        {
            foreach (DataFormat format in item.Formats)
            {
                if (Array.IndexOf(identifiers, format.Identifier) < 0)
                {
                    continue;
                }
                object? raw;
                try
                {
                    raw = item.TryGetRaw(format);
                }
                catch (Exception)
                {
                    // A format that advertised itself but cannot actually be
                    // read: try the next one rather than failing the drop.
                    continue;
                }
                string? text = raw switch
                {
                    byte[] bytes => Encoding.UTF8.GetString(bytes),
                    string s => s,
                    _ => null,
                };
                if (!string.IsNullOrEmpty(text))
                {
                    yield return text;
                }
            }
        }
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
