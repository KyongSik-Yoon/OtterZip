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

        // 2) A raw uri-list under a desktop-specific format (KDE especially).
        if (paths.Count == 0)
        {
            foreach (string uriList in RawUriLists(data))
            {
                AddUriListPaths(uriList, paths);
            }
        }

        // 3) Last resort: a plain-text payload of URIs or bare paths.
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
    /// Comma-separated format identifiers the drop carried, for the
    /// "drop produced nothing" diagnostic line.
    /// </summary>
    public static string DescribeFormats(IDataTransfer data)
    {
        var names = new List<string>();
        foreach (DataFormat format in data.Formats)
        {
            names.Add(format.Identifier);
        }
        return names.Count > 0 ? string.Join(", ", names) : "(none)";
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
    /// Every uri-list-shaped payload the drop carries, decoded from each item's
    /// raw value. The lookup uses the actual <see cref="DataFormat"/> object
    /// from the drop, so its kind (platform vs application) matches and
    /// <see cref="IDataTransferItem.TryGetRaw"/> does not miss.
    /// </summary>
    private static IEnumerable<string> RawUriLists(IDataTransfer data)
    {
        foreach (IDataTransferItem item in data.Items)
        {
            foreach (DataFormat format in item.Formats)
            {
                if (Array.IndexOf(UriListFormats, format.Identifier) < 0)
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
