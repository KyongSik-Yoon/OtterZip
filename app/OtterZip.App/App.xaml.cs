using System;
using System.Collections.Generic;
using System.Linq;
using Microsoft.UI.Xaml;
using OtterZip.App.Services;
using OtterZip.Interop;
using Windows.Globalization;

namespace OtterZip.App;

public partial class App : Application
{
    private Window? _mainWindow;

    /// <summary>
    /// The currently active main window, or <see langword="null"/> before
    /// <see cref="OnLaunched"/> fires. Sub-dialogs use this to root file
    /// pickers — picker WinRT APIs need a window handle.
    /// </summary>
    public static Window? HostWindow { get; private set; }

    public App()
    {
        InitializeComponent();
        // Surface unhandled XAML exceptions to the debugger output and
        // (when running attached) the Output window — without this the
        // WinAppSDK runtime swallows the inner Exception and the
        // breakpoint stops at `UnhandledExceptionEventArgs` with no
        // message, which is exactly the failure mode that hid the
        // SetIcon path-resolution bug. Marking Handled=true keeps the
        // process alive for any non-fatal regressions; truly fatal
        // failures (StackOverflow / AccessViolation) bypass this anyway.
        UnhandledException += (_, e) =>
        {
            System.Diagnostics.Debug.WriteLine(
                "[OtterZip] UnhandledException: " + e.Message + "\n" + e.Exception);
            e.Handled = true;
        };
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        OtterzipLibrary.Initialize();
        // Phase 7 (PR-7F): telemetry pipeline. Honors the persisted user
        // preference first; OTTERZIP_TELEMETRY=1 env var inside Initialize()
        // is a fallback for CI / power users. Default is OFF.
        OtterzipTelemetry.Initialize();
        if (SettingsService.Get<bool>("Settings_TelemetryEnabled", false))
        {
            OtterzipTelemetry.SetUserOptIn(true);
        }

        // Phase 7 (PR-7C): one-time credential migration from
        // SettingsService LocalSettings into Windows PasswordVault.
        // Idempotent on subsequent launches.
        CredentialStore.MigrateFromSettingsServiceOnce();

        // Phase 6+: honor saved language preference before any UI loads
        // resources. Empty string keeps system default, "ko" / "en" override.
        //
        // ApplicationLanguages.PrimaryLanguageOverride throws
        // `InvalidOperationException` ("Operation is not valid due to the
        // current state of the object") in unpackaged WinUI 3 contexts —
        // the API is package-identity gated. Our csproj currently runs
        // unpackaged (WindowsPackageType=None) so swallow and fall back
        // to the system locale.
        ApplyLanguageOverride();

        // Sprint 5: shell extension routes context-menu verbs through
        // `OtterZip.exe --invoke <verb> --files "..."`. We parse the
        // command line up front so the invoking workflow can complete
        // without showing the main window when appropriate.
        var invokeRequest = ParseInvokeArgs(Environment.GetCommandLineArgs());

        // Bandizip-style quick verbs (`compress-zip` / `compress-7z`)
        // land here with IsHeadless=true. We skip MainWindow entirely
        // and route to a standalone ProgressDialog so the user gets
        // the small modal experience instead of the full app surface.
        // The dialog owns its own JobQueue + dispatcher; WinUI keeps
        // the app alive until the last window closes, so when the
        // dialog hits its Close button the process exits naturally.
        if (invokeRequest is { IsHeadless: true })
        {
            var dialog = new OtterZip.App.Modals.ProgressDialog(invokeRequest);
            HostWindow = dialog;
            dialog.Activate();
            return;
        }

        _mainWindow = new MainWindow();
        HostWindow = _mainWindow;
        if (invokeRequest is not null)
        {
            ((MainWindow)_mainWindow).PreloadInvoke(invokeRequest);
        }
        _mainWindow.Activate();
    }

    /// <summary>
    /// Apply the persisted language preference to
    /// <c>ApplicationLanguages.PrimaryLanguageOverride</c> so MRT loads
    /// the matching .resw folder. We persist a short tag ("ko" / "zh" /
    /// "pt") in Settings_Language; this method maps to the full IETF
    /// tag ("ko-KR" / "zh-CN" / "pt-BR") that the runtime expects.
    /// Mirrors GeneralSettingsSection.ShortTagToIetf — keep them in sync.
    ///
    /// Best-effort: PrimaryLanguageOverride is package-identity gated
    /// and throws InvalidOperationException / COMException in unpackaged
    /// dev runs. Falling back to the system locale is the safe degrade.
    /// </summary>
    private static void ApplyLanguageOverride()
    {
        try
        {
            string shortTag = SettingsService.Get<string>("Settings_Language", "");
            ApplicationLanguages.PrimaryLanguageOverride = shortTag switch
            {
                "ko" => "ko-KR",
                "en" => "en-US",
                "zh" => "zh-CN",
                "ja" => "ja-JP",
                "de" => "de-DE",
                "fr" => "fr-FR",
                "es" => "es-ES",
                "pt" => "pt-BR",
                "ru" => "ru-RU",
                "it" => "it-IT",
                _    => "",   // empty -> system default
            };
        }
        catch (InvalidOperationException) { /* unpackaged dev */ }
        catch (System.Runtime.InteropServices.COMException) { /* same */ }
    }

    /// <summary>
    /// Parse <c>--invoke &lt;verb&gt; --files "p1;p2;..."</c>. Returns
    /// <see langword="null"/> if the args don't carry a verb (normal launch).
    ///
    /// <para>
    /// Format-specific quick verbs (<c>compress-zip</c>, <c>compress-7z</c>)
    /// collapse to <c>Verb="compress"</c> + <c>QuickFormat=&lt;tag&gt;</c>
    /// + <c>IsHeadless=true</c> so MainWindow's verb dispatch only has
    /// to compare against the canonical roots.
    /// </para>
    /// </summary>
    internal static InvokeRequest? ParseInvokeArgs(string[] argv)
    {
        if (argv is null || argv.Length < 3)
        {
            return null;
        }
        string? verb = null;
        string? files = null;
        for (int i = 0; i < argv.Length - 1; i++)
        {
            if (string.Equals(argv[i], "--invoke", StringComparison.OrdinalIgnoreCase))
            {
                verb = argv[i + 1];
            }
            else if (string.Equals(argv[i], "--files", StringComparison.OrdinalIgnoreCase))
            {
                files = argv[i + 1];
            }
        }
        if (verb is null || files is null)
        {
            return null;
        }
        var paths = files
            .Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .Where(p => p.Length > 0)
            .ToList();
        if (paths.Count == 0)
        {
            return null;
        }
        var (canonicalVerb, quickFormat, isHeadless) = NormalizeVerb(verb);
        return new InvokeRequest(canonicalVerb, paths, quickFormat, isHeadless);
    }

    /// <summary>
    /// Map a raw shell-extension verb string into the
    /// <see cref="InvokeRequest"/> shape: canonical verb root + optional
    /// quick-format tag + headless flag. Extracted from
    /// <see cref="ParseInvokeArgs"/> to keep that method under the
    /// MA0051 60-line cap (2026-05-19 4-context parity sprint added 4
    /// new verbs, which pushed it over).
    ///
    /// <para>
    /// Recognised verbs:
    /// <list type="bullet">
    /// <item><c>compress-zip</c> / <c>compress-7z</c> — collapse to
    /// <c>compress</c> + <c>QuickFormat</c>, headless (single-archive
    /// ProgressDialog path).</item>
    /// <item><c>compress-individually</c> — N-archive emit. NOT headless:
    /// the ProgressDialog builds a single CompressEngine plan, so to
    /// produce one archive per item we must route through
    /// MainWindow.DispatchInvokeAsync's per-path loop.</item>
    /// <item><c>extract-smart</c> / <c>extract-to-subfolder</c> /
    /// <c>extract-dialog</c> — all EXTRACT operations. NOT headless:
    /// the headless ProgressDialog only knows how to compress
    /// (CompressEngine), so an extract verb sent there would silently
    /// COMPRESS the archive instead of unpacking it (the 2026-06-17
    /// bug). They route through MainWindow.DispatchInvokeAsync →
    /// ExtractAsync with the appropriate ShellExtractMode.</item>
    /// <item>Unrecognised → returned verbatim so MainWindow dispatch
    /// can produce a useful error.</item>
    /// </list>
    ///
    /// Headless == true is reserved for verbs the standalone
    /// ProgressDialog can fully service today, which is only the two
    /// single-archive quick-compress verbs. Everything else needs
    /// MainWindow's richer dispatch. v0.22's dedicated
    /// Compress/Extract option dialogs will revisit this split.
    /// </para>
    /// </summary>
    private static (string canonical, string? quickFormat, bool isHeadless) NormalizeVerb(string verb)
    {
        if (string.Equals(verb, "compress-zip", StringComparison.OrdinalIgnoreCase))
        {
            return ("compress", "zip", true);
        }
        if (string.Equals(verb, "compress-7z", StringComparison.OrdinalIgnoreCase))
        {
            return ("compress", "7z", true);
        }
        if (string.Equals(verb, "compress-individually", StringComparison.OrdinalIgnoreCase))
        {
            // NOT headless — needs the per-path N-archive loop in
            // DispatchInvokeAsync. Headless would make a single combined
            // archive, defeating "각 항목별 압축".
            return ("compress-individually", null, false);
        }
        if (string.Equals(verb, "extract-smart", StringComparison.OrdinalIgnoreCase))
        {
            // NOT headless — extract, not compress. See class remarks.
            return ("extract-smart", null, false);
        }
        if (string.Equals(verb, "extract-to-subfolder", StringComparison.OrdinalIgnoreCase))
        {
            return ("extract-to-subfolder", null, false);
        }
        if (string.Equals(verb, "extract-dialog", StringComparison.OrdinalIgnoreCase))
        {
            return ("extract-dialog", null, false);
        }
        return (verb, null, false);
    }
}
