// OtterZip for Linux — desktop integration.
//
// The Linux replacement for app/OtterZip.Shell (the C++/WinRT IExplorerCommand
// in-proc server). There is no cross-desktop context-menu API on Linux, so
// "right-click → extract" is assembled from three separate freedesktop
// mechanisms, each of which every major desktop implements:
//
//   1. `.desktop` entries under $XDG_DATA_HOME/applications declare OtterZip
//      as an application that can open archives. That is what puts it in
//      "Open With", and what makes double-clicking an archive work.
//   2. MIME associations (mimeapps.list) make OtterZip the DEFAULT handler
//      for the archive types it can open.
//   3. Per-file-manager action files add the actual verbs ("Extract here",
//      "Compress to zip") to the right-click menu. The formats differ by
//      file manager, so each gets its own writer below.
//
// Everything is written under the user's home — no root, no package manager,
// no polkit prompt — and Uninstall removes exactly what Install wrote.

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Text;

namespace OtterZip.App.Services;

/// <summary>
/// Installs and removes the desktop-level integration: application entries,
/// MIME defaults, and file-manager context-menu actions.
/// </summary>
public static class DesktopIntegration
{
    /// <summary>Reverse-DNS id used for the main `.desktop` file.</summary>
    private const string AppId = "io.github.lumibearstudio.OtterZip";

    /// <summary>
    /// MIME types OtterZip registers as a handler for. Restricted to formats
    /// the core can actually OPEN — registering for a type we would then
    /// refuse is worse than not appearing in the menu at all.
    /// </summary>
    /// <remarks>
    /// Both the modern <c>application/vnd.*</c> / <c>application/x-*</c>
    /// spellings and the legacy aliases appear, because which one a file
    /// manager reports depends on its shared-mime-info version.
    /// </remarks>
    private static readonly string[] ArchiveMimeTypes =
    [
        "application/zip",
        "application/x-zip-compressed",
        "application/x-7z-compressed",
        "application/vnd.rar",
        "application/x-rar-compressed",
        "application/x-tar",
        "application/gzip",
        "application/x-gzip",
        "application/x-compressed-tar",
        "application/x-bzip",
        "application/x-bzip2",
        "application/x-bzip-compressed-tar",
        "application/x-xz",
        "application/x-xz-compressed-tar",
        "application/zstd",
        "application/x-zstd-compressed-tar",
        "application/x-lz4",
        "application/x-lzma",
        "application/x-cd-image",
        "application/vnd.ms-cab-compressed",
        "application/x-msi",
        "application/vnd.debian.binary-package",
        "application/java-archive",
        "application/vnd.android.package-archive",
    ];

    /// <summary>
    /// Where the integration files live. Everything is under the user's data
    /// directory, so install/uninstall never needs elevation.
    /// </summary>
    private static string DataHome
    {
        get
        {
            string? xdg = Environment.GetEnvironmentVariable("XDG_DATA_HOME");
            return !string.IsNullOrEmpty(xdg) && Path.IsPathRooted(xdg)
                ? xdg
                : Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                    ".local", "share");
        }
    }

    /// <summary>
    /// <c>$XDG_CONFIG_HOME</c>, falling back to <c>~/.config</c>. This is the
    /// PARENT of SettingsService.ConfigDirectory — mimeapps.list and Thunar's
    /// uca.xml are desktop-owned files that live beside our own directory,
    /// not inside it.
    /// </summary>
    private static string ConfigHome
    {
        get
        {
            string? xdg = Environment.GetEnvironmentVariable("XDG_CONFIG_HOME");
            return !string.IsNullOrEmpty(xdg) && Path.IsPathRooted(xdg)
                ? xdg
                : Path.Combine(
                    Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                    ".config");
        }
    }

    private static string ApplicationsDir => Path.Combine(DataHome, "applications");

    /// <summary>
    /// Absolute path of the running executable, used as the <c>Exec=</c>
    /// target. Taken from the process rather than
    /// <see cref="AppContext.BaseDirectory"/> so a single-file publish and a
    /// framework-dependent layout both produce a launchable command.
    /// </summary>
    public static string ExecutablePath
    {
        get
        {
            string? main = Environment.ProcessPath;
            if (!string.IsNullOrEmpty(main))
            {
                return main;
            }
            return Path.Combine(AppContext.BaseDirectory, "otterzip-gui");
        }
    }

    /// <summary>
    /// Whether the integration appears to be installed. Checked by Settings →
    /// Integration to label its button "Install" or "Remove".
    /// </summary>
    public static bool IsInstalled =>
        File.Exists(Path.Combine(ApplicationsDir, AppId + ".desktop"));

    /// <summary>
    /// Write every integration file and refresh the desktop caches.
    /// </summary>
    /// <returns>
    /// A human-readable log of what was written, shown in the Settings pane
    /// so the user can see exactly what the button touched.
    /// </returns>
    public static string Install()
    {
        var log = new StringBuilder();
        string exec = ExecutablePath;
        string icon = InstallIcon(log);

        Directory.CreateDirectory(ApplicationsDir);

        // 1. The application entry. NoDisplay is false: OtterZip is a real
        //    windowed app, so it belongs in the launcher too.
        string mainDesktop = Path.Combine(ApplicationsDir, AppId + ".desktop");
        Write(mainDesktop, BuildMainDesktopEntry(exec, icon), log);

        // 2. The verb entries. These are hidden from the launcher
        //    (NoDisplay=true) — they exist only to be referenced by
        //    "Open With" and by the file-manager actions below, and a user
        //    who found "Extract here" in their app grid would be confused.
        Write(
            Path.Combine(ApplicationsDir, AppId + ".ExtractHere.desktop"),
            BuildVerbDesktopEntry(exec, icon, "Shell_ExtractHere_Title", "--invoke", "extract-here"),
            log);
        Write(
            Path.Combine(ApplicationsDir, AppId + ".ExtractTo.desktop"),
            BuildVerbDesktopEntry(exec, icon, "Shell_ExtractToSubfolder_Tooltip", "--invoke", "extract-to-subfolder"),
            log);

        WriteMimeDefaults(log);
        WriteNautilusScripts(exec, log);
        WriteThunarActions(exec, log);
        WriteDolphinServiceMenu(exec, icon, log);
        RefreshCaches(log);

        return log.ToString();
    }

    /// <summary>
    /// Remove every file <see cref="Install"/> wrote. Leaves the user's
    /// settings alone — uninstalling the menu integration is not a request to
    /// forget their preferences.
    /// </summary>
    public static string Uninstall()
    {
        var log = new StringBuilder();
        foreach (string path in InstalledPaths())
        {
            try
            {
                if (File.Exists(path))
                {
                    File.Delete(path);
                    log.Append("removed ").Append(path).Append('\n');
                }
            }
            catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
            {
                log.Append("could not remove ").Append(path).Append(": ").Append(ex.Message).Append('\n');
            }
        }
        RefreshCaches(log);
        return log.ToString();
    }

    private static IEnumerable<string> InstalledPaths()
    {
        yield return Path.Combine(ApplicationsDir, AppId + ".desktop");
        yield return Path.Combine(ApplicationsDir, AppId + ".ExtractHere.desktop");
        yield return Path.Combine(ApplicationsDir, AppId + ".ExtractTo.desktop");
        yield return Path.Combine(DataHome, "icons", "hicolor", "256x256", "apps", "otterzip.png");
        yield return Path.Combine(DataHome, "nautilus", "scripts", "OtterZip — Extract here");
        yield return Path.Combine(DataHome, "nautilus", "scripts", "OtterZip — Compress");
        yield return Path.Combine(DataHome, "kio", "servicemenus", "otterzip.desktop");
        yield return Path.Combine(ConfigHome, "Thunar", "uca.xml.otterzip");
    }

    private static string BuildMainDesktopEntry(string exec, string icon)
    {
        var sb = new StringBuilder()
            .Append("[Desktop Entry]\n")
            .Append("Type=Application\n")
            .Append("Version=1.5\n")
            .Append("Name=OtterZip\n")
            .Append("GenericName=Archive Manager\n")
            .Append("Comment=").Append(Sanitize(Strings.Get("Shell_OtterzipMenu_Tooltip"))).Append('\n')
            // %F, not %f: OtterZip accepts a multi-file selection and
            // compresses it into one archive. With %f the file manager would
            // launch one process per selected file and produce N archives.
            .Append("Exec=").Append(Quote(exec)).Append(" %F\n")
            .Append("Icon=").Append(icon).Append('\n')
            .Append("Terminal=false\n")
            .Append("Categories=Utility;Archiving;Compression;\n")
            .Append("Keywords=zip;7z;rar;tar;archive;compress;extract;\n")
            .Append("StartupNotify=true\n")
            .Append("StartupWMClass=otterzip-gui\n")
            .Append("MimeType=").Append(string.Join(';', ArchiveMimeTypes)).Append(";\n")
            // Desktop actions: these show up on the launcher icon's own
            // right-click menu (dock / app grid), which is the closest thing
            // to the Windows jump list.
            .Append("Actions=ExtractHere;ExtractTo;\n\n")
            .Append("[Desktop Action ExtractHere]\n")
            .Append("Name=").Append(Sanitize(Strings.Get("Shell_ExtractHereSubmenu_Title"))).Append('\n')
            .Append("Exec=").Append(Quote(exec)).Append(" --invoke extract-here --files %F\n\n")
            .Append("[Desktop Action ExtractTo]\n")
            .Append("Name=").Append(Sanitize(Strings.Get("Shell_ExtractSmart_Title"))).Append('\n')
            .Append("Exec=").Append(Quote(exec)).Append(" --invoke extract-smart --files %F\n");
        return sb.ToString();
    }

    private static string BuildVerbDesktopEntry(
        string exec, string icon, string nameKey, string flag, string verb)
    {
        return new StringBuilder()
            .Append("[Desktop Entry]\n")
            .Append("Type=Application\n")
            .Append("Name=OtterZip — ").Append(Sanitize(Strings.Get(nameKey))).Append('\n')
            .Append("Exec=").Append(Quote(exec)).Append(' ').Append(flag).Append(' ')
            .Append(verb).Append(" --files %F\n")
            .Append("Icon=").Append(icon).Append('\n')
            .Append("Terminal=false\n")
            // Hidden from the launcher: reachable through "Open With" and the
            // file-manager actions, not something to browse to.
            .Append("NoDisplay=true\n")
            .Append("MimeType=").Append(string.Join(';', ArchiveMimeTypes)).Append(";\n")
            .ToString();
    }

    /// <summary>
    /// Make OtterZip the default handler for every archive type it supports,
    /// by merging into <c>mimeapps.list</c> rather than rewriting it — that
    /// file also holds the user's choices for every OTHER file type, and
    /// clobbering it would reset their whole desktop.
    /// </summary>
    private static void WriteMimeDefaults(StringBuilder log)
    {
        string path = Path.Combine(ConfigHome, "mimeapps.list");
        try
        {
            Directory.CreateDirectory(ConfigHome);
            var lines = new List<string>();
            if (File.Exists(path))
            {
                lines.AddRange(File.ReadAllLines(path));
            }

            int header = lines.FindIndex(l =>
                string.Equals(l.Trim(), "[Default Applications]", StringComparison.Ordinal));
            if (header < 0)
            {
                lines.Add("[Default Applications]");
                header = lines.Count - 1;
            }

            string desktopFile = AppId + ".desktop";
            foreach (string mime in ArchiveMimeTypes)
            {
                string entry = mime + "=" + desktopFile;
                int existing = lines.FindIndex(
                    l => l.StartsWith(mime + "=", StringComparison.Ordinal));
                if (existing >= 0)
                {
                    lines[existing] = entry;
                }
                else
                {
                    lines.Insert(header + 1, entry);
                }
            }
            File.WriteAllLines(path, lines);
            log.Append("updated ").Append(path).Append('\n');
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            log.Append("could not update ").Append(path).Append(": ").Append(ex.Message).Append('\n');
        }
    }

    /// <summary>
    /// Nautilus (GNOME Files) reads executable scripts from
    /// <c>$XDG_DATA_HOME/nautilus/scripts</c> and lists them under
    /// "Scripts" in the right-click menu, passing the selection in
    /// <c>$NAUTILUS_SCRIPT_SELECTED_FILE_PATHS</c> — one path per line.
    /// </summary>
    private static void WriteNautilusScripts(string exec, StringBuilder log)
    {
        string dir = Path.Combine(DataHome, "nautilus", "scripts");
        // Newline-separated selection → one --files argument per path. `IFS=`
        // and `read -r` keep spaces and backslashes in filenames intact,
        // which naive `for f in $(...)` would destroy.
        // Written as a plain (non-interpolated) raw string with %TOKEN%
        // placeholders: the script is dense with `$` and `{}`, and every one
        // of them would have to be escaped in an interpolated literal, which
        // is exactly how a shell script picks up a subtle quoting bug.
        const string Template = """
            #!/bin/sh
            # Generated by OtterZip — do not edit; reinstall from Settings → Integration.
            set -eu
            # Nautilus passes the selection as one path per line. `IFS=` plus
            # `read -r` keeps spaces, tabs and backslashes in filenames intact,
            # which a naive `for f in $(...)` would split and mangle.
            while IFS= read -r p; do
                [ -n "$p" ] || continue
                set -- "$@" "$p"
            done <<EOF
            ${NAUTILUS_SCRIPT_SELECTED_FILE_PATHS:-}
            EOF
            [ "$#" -gt 0 ] || exit 0
            exec %EXEC% --invoke %VERB% --files "$@"
            """;

        string body = Template.Replace("%EXEC%", Quote(exec), StringComparison.Ordinal);
        WriteScript(
            Path.Combine(dir, "OtterZip — Extract here"),
            body.Replace("%VERB%", "extract-here", StringComparison.Ordinal),
            log);
        WriteScript(
            Path.Combine(dir, "OtterZip — Compress"),
            body.Replace("%VERB%", "compress", StringComparison.Ordinal),
            log);
    }

    /// <summary>
    /// Thunar stores custom actions in a single <c>uca.xml</c> it rewrites
    /// wholesale, so merging into it risks losing the user's own actions if
    /// Thunar is running. Write a fragment beside it instead and tell the
    /// user how to merge — honest beats clever here.
    /// </summary>
    private static void WriteThunarActions(string exec, StringBuilder log)
    {
        string dir = Path.Combine(ConfigHome, "Thunar");
        string path = Path.Combine(dir, "uca.xml.otterzip");
        string body = $"""
            <!-- OtterZip actions. Thunar rewrites uca.xml wholesale while it is
                 running, so these are not merged automatically: paste the two
                 <action> blocks into ~/.config/Thunar/uca.xml (inside <actions>)
                 with Thunar closed, or add them via Edit → Configure custom actions. -->
            <action>
              <icon>otterzip</icon>
              <name>{Escape(Strings.Get("Shell_ExtractHere_Title"))}</name>
              <command>{Escape(exec)} --invoke extract-here --files %F</command>
              <patterns>*.zip;*.7z;*.rar;*.tar;*.gz;*.tgz;*.bz2;*.xz;*.zst;*.lz4;*.iso;*.cab;*.deb;*.jar;*.apk</patterns>
              <other-files/>
            </action>
            <action>
              <icon>otterzip</icon>
              <name>{Escape(Strings.Get("Shell_CompressDialog_Title"))}</name>
              <command>{Escape(exec)} --invoke compress --files %F</command>
              <patterns>*</patterns>
              <directories/>
              <other-files/>
            </action>
            """;
        try
        {
            Directory.CreateDirectory(dir);
            File.WriteAllText(path, body);
            log.Append("wrote ").Append(path).Append(" (manual merge — see the comment inside)\n");
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            log.Append("could not write ").Append(path).Append(": ").Append(ex.Message).Append('\n');
        }
    }

    /// <summary>
    /// Dolphin (KDE) reads service menus from
    /// <c>$XDG_DATA_HOME/kio/servicemenus</c>. Unlike Nautilus scripts these
    /// are declarative and support a submenu, so this is the one file manager
    /// where the menu shape actually matches the Windows shell extension's.
    /// </summary>
    private static void WriteDolphinServiceMenu(string exec, string icon, StringBuilder log)
    {
        string dir = Path.Combine(DataHome, "kio", "servicemenus");
        string path = Path.Combine(dir, "otterzip.desktop");
        string archiveMimes = string.Join(';', ArchiveMimeTypes);
        string body = new StringBuilder()
            .Append("[Desktop Entry]\n")
            .Append("Type=Service\n")
            .Append("ServiceTypes=KonqPopupMenu/Plugin\n")
            .Append("MimeType=").Append(archiveMimes).Append(";inode/directory;application/octet-stream;\n")
            .Append("Icon=").Append(icon).Append('\n')
            .Append("X-KDE-Submenu=OtterZip\n")
            .Append("Actions=ExtractHere;ExtractSmart;CompressZip;Compress7z;\n\n")
            .Append("[Desktop Action ExtractHere]\n")
            .Append("Name=").Append(Sanitize(Strings.Get("Shell_ExtractHere_Title"))).Append('\n')
            .Append("Icon=").Append(icon).Append('\n')
            .Append("Exec=").Append(Quote(exec)).Append(" --invoke extract-here --files %F\n\n")
            .Append("[Desktop Action ExtractSmart]\n")
            .Append("Name=").Append(Sanitize(Strings.Get("Shell_ExtractSmart_Title"))).Append('\n')
            .Append("Icon=").Append(icon).Append('\n')
            .Append("Exec=").Append(Quote(exec)).Append(" --invoke extract-smart --files %F\n\n")
            .Append("[Desktop Action CompressZip]\n")
            .Append("Name=").Append(Sanitize(Strings.Get("Shell_CompressZipQuick_Tooltip"))).Append('\n')
            .Append("Icon=").Append(icon).Append('\n')
            .Append("Exec=").Append(Quote(exec)).Append(" --invoke compress-zip --files %F\n\n")
            .Append("[Desktop Action Compress7z]\n")
            .Append("Name=").Append(Sanitize(Strings.Get("Shell_Compress7zQuick_Tooltip"))).Append('\n')
            .Append("Icon=").Append(icon).Append('\n')
            .Append("Exec=").Append(Quote(exec)).Append(" --invoke compress-7z --files %F\n")
            .ToString();
        Write(path, body, log);
    }

    /// <summary>
    /// Copy the app icon into the hicolor theme so `Icon=otterzip` resolves
    /// in every menu that renders one. Falls back to the literal executable
    /// path's icon name when the source PNG is missing (a trimmed publish).
    /// </summary>
    private static string InstallIcon(StringBuilder log)
    {
        string source = Path.Combine(AppContext.BaseDirectory, "Assets", "otterzip.png");
        string dir = Path.Combine(DataHome, "icons", "hicolor", "256x256", "apps");
        string target = Path.Combine(dir, "otterzip.png");
        try
        {
            if (File.Exists(source))
            {
                Directory.CreateDirectory(dir);
                File.Copy(source, target, overwrite: true);
                log.Append("wrote ").Append(target).Append('\n');
                return "otterzip";
            }
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            log.Append("could not install icon: ").Append(ex.Message).Append('\n');
        }
        // A generic themed icon is better than a missing one.
        return "package-x-generic";
    }

    /// <summary>
    /// Tell the desktop to re-read what we just wrote. Both tools are
    /// optional: without them the new entries appear on next login instead of
    /// immediately, which is a delay, not a failure.
    /// </summary>
    private static void RefreshCaches(StringBuilder log)
    {
        if (Run("update-desktop-database", ApplicationsDir))
        {
            log.Append("refreshed desktop database\n");
        }
        if (Run("gtk-update-icon-cache", "-f", "-t", Path.Combine(DataHome, "icons", "hicolor")))
        {
            log.Append("refreshed icon cache\n");
        }
    }

    private static void Write(string path, string content, StringBuilder log)
    {
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.WriteAllText(path, content);
            log.Append("wrote ").Append(path).Append('\n');
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException)
        {
            log.Append("could not write ").Append(path).Append(": ").Append(ex.Message).Append('\n');
        }
    }

    private static void WriteScript(string path, string content, StringBuilder log)
    {
        Write(path, content, log);
        try
        {
            // Nautilus only lists scripts that are executable. The guard is
            // for the analyzer's benefit: this whole class is Linux-only, but
            // the assembly itself targets plain net9.0.
            if (File.Exists(path) && !OperatingSystem.IsWindows())
            {
                File.SetUnixFileMode(
                    path,
                    UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute
                    | UnixFileMode.GroupRead | UnixFileMode.GroupExecute
                    | UnixFileMode.OtherRead | UnixFileMode.OtherExecute);
            }
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or PlatformNotSupportedException)
        {
            log.Append("could not mark ").Append(path).Append(" executable: ").Append(ex.Message).Append('\n');
        }
    }

    private static bool Run(string file, params string[] args)
    {
        try
        {
            var psi = new ProcessStartInfo { FileName = file, UseShellExecute = false, RedirectStandardOutput = true, RedirectStandardError = true };
            foreach (string a in args)
            {
                psi.ArgumentList.Add(a);
            }
            using Process? p = Process.Start(psi);
            if (p is null)
            {
                return false;
            }
            return p.WaitForExit(10_000) && p.ExitCode == 0;
        }
        catch (Exception ex) when (ex is System.ComponentModel.Win32Exception or InvalidOperationException)
        {
            return false;
        }
    }

    /// <summary>
    /// Quote a path for a `.desktop` <c>Exec=</c> line. The spec requires
    /// double quotes around any value containing a space and backslash
    /// escaping of <c>"</c>, <c>`</c>, <c>$</c> and <c>\</c> inside them.
    /// </summary>
    private static string Quote(string path)
    {
        if (path.IndexOfAny([' ', '\t', '"', '\'', '\\', '$', '`']) < 0)
        {
            return path;
        }
        var sb = new StringBuilder("\"");
        foreach (char c in path)
        {
            if (c is '"' or '`' or '$' or '\\')
            {
                sb.Append('\\');
            }
            sb.Append(c);
        }
        return sb.Append('"').ToString();
    }

    /// <summary>
    /// Strip the Windows accelerator markers (<c>&amp;X</c>) and any newline
    /// out of a catalogue string before it goes into a key=value line — the
    /// `.desktop` format is line-oriented and has no notion of accelerators.
    /// </summary>
    private static string Sanitize(string value)
    {
        var sb = new StringBuilder(value.Length);
        foreach (char c in value)
        {
            if (c is '&' or '\n' or '\r')
            {
                continue;
            }
            sb.Append(c);
        }
        // Drop a trailing " (X)" accelerator hint left behind by the removal.
        string s = sb.ToString().Trim();
        int paren = s.LastIndexOf(" (", StringComparison.Ordinal);
        if (paren > 0 && s.EndsWith(')') && s.Length - paren <= 5)
        {
            s = s[..paren];
        }
        return s;
    }

    private static string Escape(string value) =>
        value.Replace("&", "&amp;", StringComparison.Ordinal)
             .Replace("<", "&lt;", StringComparison.Ordinal)
             .Replace(">", "&gt;", StringComparison.Ordinal);

    /// <summary>
    /// Human-readable one-liner for the Settings pane, e.g.
    /// "Installed — 7 files under ~/.local/share".
    /// </summary>
    public static string DescribeState() => string.Format(
        CultureInfo.CurrentCulture,
        IsInstalled ? "{0} — {1}" : "{0}",
        IsInstalled ? Strings.Get("Settings_ShellIntegration_On") : Strings.Get("Settings_ShellIntegration_Off"),
        ApplicationsDir);
}
