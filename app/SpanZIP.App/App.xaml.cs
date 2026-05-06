using System;
using System.Collections.Generic;
using System.Linq;
using Microsoft.UI.Xaml;
using SpanZIP.App.Services;
using SpanZIP.Interop;
using Windows.Globalization;

namespace SpanZIP.App;

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
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        SpanzipLibrary.Initialize();
        // Phase 7 (PR-7F): telemetry pipeline. Honors the persisted user
        // preference first; SPANZIP_TELEMETRY=1 env var inside Initialize()
        // is a fallback for CI / power users. Default is OFF.
        SpanzipTelemetry.Initialize();
        if (SettingsService.Get<bool>("Settings_TelemetryEnabled", false))
        {
            SpanzipTelemetry.SetUserOptIn(true);
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
        try
        {
            ApplicationLanguages.PrimaryLanguageOverride =
                SettingsService.Get<string>("Settings_Language", "");
        }
        catch (InvalidOperationException)
        {
            // Unpackaged dev / sideloaded run — language follows OS.
        }
        catch (System.Runtime.InteropServices.COMException)
        {
            // Same surface as above on some Windows builds.
        }

        // Sprint 5: shell extension routes context-menu verbs through
        // `SpanZIP.exe --invoke <verb> --files "..."`. We parse the
        // command line up front so the invoking workflow can complete
        // without showing the main window when appropriate.
        var invokeRequest = ParseInvokeArgs(Environment.GetCommandLineArgs());

        _mainWindow = new MainWindow();
        HostWindow = _mainWindow;
        if (invokeRequest is not null)
        {
            ((MainWindow)_mainWindow).PreloadInvoke(invokeRequest);
        }
        _mainWindow.Activate();
    }

    /// <summary>
    /// Parse <c>--invoke &lt;verb&gt; --files "p1;p2;..."</c>. Returns
    /// <see langword="null"/> if the args don't carry a verb (normal launch).
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
        // Verbs are ASCII tokens in our control (extract-here / compress / etc.)
        // We compare case-insensitively in the consumer, so just preserve as-is.
        return new InvokeRequest(verb, paths);
    }
}
