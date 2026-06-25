using System;
using System.IO;
using System.Text;
using Microsoft.Windows.ApplicationModel.Resources;
using Windows.Storage;

namespace OtterZip.App.Services;

/// <summary>
/// Mirrors the values the shell-extension menu-build path needs (the two
/// integration toggles, the menu-mode string, and every localized
/// <c>Shell_*</c> verb title/tooltip) into a small file in the package
/// LocalState folder.
///
/// <para>Why: the packaged <c>IExplorerCommand</c> handler reads these via
/// WinRT (<c>LocalSettings</c> / <c>ResourceLoader</c> /
/// <c>Package.InstalledLocation</c>), which forces Explorer to establish
/// the AppX activation context on the first packaged-handler use after
/// boot. That one-time cost exceeds the Windows 11 modern context-menu
/// timeout, so our verbs are dropped from the first right-click and only
/// appear on the second. The shell's <c>ShellCache</c> reads this file
/// with pure Win32 (no AppX context), so once this mirror exists the first
/// right-click after every boot shows the verbs.</para>
///
/// <para>The file lives in <see cref="ApplicationData"/>.LocalFolder
/// (= <c>%LOCALAPPDATA%\Packages\&lt;PFN&gt;\LocalState</c>), the canonical
/// shared store between the packaged app and the packaged shell handler.
/// It persists across reboots, so a single prior launch is enough.</para>
/// </summary>
internal static class ShellMenuCache
{
    private const string FileName = "shell-menu-cache.v1.txt";

    // Keep in sync with the L"Shell_*" keys read in app/OtterZip.Shell/src
    // (grep `L"Shell_`). Stored verbatim; format keys keep their {0}/{1}
    // placeholders — the shell does its own runtime substitution.
    private static readonly string[] StringKeys =
    {
        "Shell_OtterzipMenu_Title", "Shell_OtterzipMenu_Tooltip",
        "Shell_CompressDialog_Title", "Shell_CompressDialog_Tooltip",
        "Shell_CompressQuick_TitleFormat",
        "Shell_CompressZipQuick_Tooltip", "Shell_Compress7zQuick_Tooltip",
        "Shell_CompressIndividually_Title", "Shell_CompressIndividually_Tooltip",
        "Shell_ExtractHere_Title", "Shell_ExtractHere_Tooltip", "Shell_ExtractHereSubmenu_Title",
        "Shell_ExtractSmart_Title", "Shell_ExtractSmart_Tooltip", "Shell_ExtractSmartEach_Title",
        "Shell_ExtractToSubfolder_TitleFormat", "Shell_ExtractToSubfolder_Tooltip", "Shell_ExtractToSubfolderEach_Title",
        "Shell_ExtractDialog_Title", "Shell_ExtractDialog_Tooltip",
        "Shell_OpenApp_Title", "Shell_OpenApp_Tooltip",
    };

    private static bool _subscribed;

    /// <summary>
    /// Rewrite the cache from current settings + the active locale's
    /// strings, and (once) subscribe to live setting/locale changes.
    /// Safe to call on every launch; never throws.
    /// </summary>
    public static void Refresh()
    {
        try
        {
            WriteCache();
        }
        catch (Exception)
        {
            // Best-effort: in unpackaged dev runs ApplicationData.Current
            // throws and the shell isn't active anyway. The shell falls
            // back to its WinRT readers when the file is absent.
        }
        EnsureLiveUpdates();
    }

    private static void WriteCache()
    {
        string localState = ApplicationData.Current.LocalFolder.Path;
        var loader = new ResourceLoader();

        var sb = new StringBuilder(2048);
        sb.Append("v1\n");
        sb.Append("ShellMenuEnabled\t")
          .Append(SettingsService.Get<bool>("Settings_ShellMenuEnabled", true) ? '1' : '0').Append('\n');
        sb.Append("ShellMenuMode\t")
          .Append(Sanitize(SettingsService.Get<string>("Settings_ShellMenuMode", "flat"))).Append('\n');

        foreach (string key in StringKeys)
        {
            string value;
            try { value = loader.GetString(key); }
            catch (Exception) { value = string.Empty; }
            if (string.IsNullOrEmpty(value)) continue;
            sb.Append(key).Append('\t').Append(Sanitize(value)).Append('\n');
        }

        string finalPath = Path.Combine(localState, FileName);
        string tmpPath = finalPath + ".tmp";
        File.WriteAllText(tmpPath, sb.ToString(), new UTF8Encoding(encoderShouldEmitUTF8Identifier: false));
        if (File.Exists(finalPath))
        {
            File.Replace(tmpPath, finalPath, destinationBackupFileName: null);
        }
        else
        {
            File.Move(tmpPath, finalPath);
        }
    }

    // Values are single-line; strip any control chars that would break the
    // key<TAB>value\n line format the shell parser expects.
    private static string Sanitize(string s)
        => s.Replace('\t', ' ').Replace('\r', ' ').Replace('\n', ' ');

    private static void EnsureLiveUpdates()
    {
        if (_subscribed) return;
        _subscribed = true;
        SettingsService.Changed += static (_, e) =>
        {
            if (e.Key is "Settings_ShellMenuEnabled"
                or "Settings_ShellMenuMode"
                or "Settings_Language")
            {
                try { WriteCache(); }
                catch (Exception) { /* best effort */ }
            }
        };
    }
}
