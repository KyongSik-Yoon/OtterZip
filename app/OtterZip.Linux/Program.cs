// OtterZip for Linux — process entry point and command-line contract.
//
// The command line is the Linux equivalent of the Windows shell extension's
// IExplorerCommand invocations: the `.desktop` files and file-manager action
// scripts written by DesktopIntegration all launch this executable with
// `--invoke <verb> --files <paths…>`. The verb vocabulary is deliberately the
// same one app/OtterZip.App/App.xaml.cs::NormalizeVerb accepts, so the two
// platforms' context menus stay describable by one document.

using System;
using System.Collections.Generic;
using System.Globalization;
using Avalonia;

namespace OtterZip.Linux;

internal static class Program
{
    /// <summary>
    /// Avalonia needs an explicit STA-free entry point that runs before any
    /// UI type is touched, hence <c>STAThread</c>'s absence and the
    /// <c>[STAThread]</c>-free signature.
    /// </summary>
    public static int Main(string[] args)
    {
        if (args.Length > 0 && IsHelpFlag(args[0]))
        {
            Console.WriteLine(UsageText());
            return 0;
        }

        App.PendingInvoke = ParseInvoke(args);
        return BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);
    }

    /// <summary>Avalonia's designer and the runtime both call this.</summary>
    public static AppBuilder BuildAvaloniaApp() =>
        AppBuilder.Configure<App>()
            .UsePlatformDetect()
            .WithInterFont()
            .LogToTrace();

    private static bool IsHelpFlag(string arg) =>
        string.Equals(arg, "--help", StringComparison.Ordinal)
        || string.Equals(arg, "-h", StringComparison.Ordinal);

    private static string UsageText() => string.Create(
        CultureInfo.InvariantCulture,
        $"""
        otterzip-gui — OtterZip graphical archive tool

        Usage:
          otterzip-gui [FILE…]
              Open the window. Archives are queued for extraction, anything
              else is queued for compression — the same rule the drop target
              uses.

          otterzip-gui --invoke VERB --files FILE [FILE…]
              Run a context-menu verb. Installed into the file manager by
              Settings → Integration.

        Verbs:
          extract-here          Extract next to the archive.
          extract-smart         Extract into a folder only when the archive
                                is not already single-rooted.
          extract-to-subfolder  Always extract into a folder named after the
                                archive.
          compress              Compress the selection with the default format.
          compress-zip          Compress the selection to ZIP.
          compress-7z           Compress the selection to 7z.
          compress-individually One archive per selected item.

        For scripting and pipelines use the `otterzip` command-line tool
        instead — it is the same engine without a display server.
        """);

    /// <summary>
    /// Parse <c>--invoke VERB --files p1 p2 …</c>, or a bare list of paths.
    /// </summary>
    /// <remarks>
    /// Unlike the Windows build, paths arrive as SEPARATE argv entries rather
    /// than one semicolon-joined string. That is not a style choice: a POSIX
    /// filename may legally contain a semicolon (and a newline, and a comma),
    /// so any in-band separator would corrupt real filenames. `%F` in a
    /// `.desktop` Exec line expands to separate arguments already, so this
    /// costs nothing.
    /// </remarks>
    private static InvokeRequest? ParseInvoke(string[] args)
    {
        string? verb = null;
        var files = new List<string>();
        bool collectingFiles = false;

        foreach (string arg in args)
        {
            if (string.Equals(arg, "--invoke", StringComparison.Ordinal))
            {
                collectingFiles = false;
                verb = string.Empty; // next non-flag token is the verb
                continue;
            }
            if (string.Equals(arg, "--files", StringComparison.Ordinal))
            {
                collectingFiles = true;
                continue;
            }
            if (verb is not null && verb.Length == 0 && !collectingFiles)
            {
                verb = arg;
                continue;
            }
            // Everything else is a path: either after --files, or a bare
            // argument list from "Open With".
            files.Add(arg);
        }

        if (files.Count == 0)
        {
            return null;
        }
        // No verb → the window classifies the drop itself, exactly as if the
        // user had dragged the files in.
        return new InvokeRequest(
            string.IsNullOrEmpty(verb) ? "open" : verb!,
            files);
    }
}

/// <summary>
/// Parsed <c>--invoke VERB --files …</c> payload.
/// </summary>
/// <param name="Verb">
/// Canonical verb, or <c>"open"</c> when the paths came in without one and
/// the window should classify them itself.
/// </param>
/// <param name="Paths">Filesystem paths supplied on the command line.</param>
public sealed record InvokeRequest(string Verb, IReadOnlyList<string> Paths);
