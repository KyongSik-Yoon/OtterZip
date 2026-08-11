// OtterZip for Linux — application shell.
//
// Mirrors app/OtterZip.App/App.xaml.cs's job: initialise the native library,
// apply the theme, then decide whether this launch shows the main window or
// runs a context-menu verb headlessly.

using System;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Markup.Xaml;
using Avalonia.Styling;
using OtterZip.App.Services;
using OtterZip.Interop;
using OtterZip.Linux.Views;

namespace OtterZip.Linux;

public partial class App : Application
{
    /// <summary>
    /// Verb + paths parsed from the command line, or <c>null</c> for a plain
    /// launch. Set by <c>Program.Main</c> before the Avalonia lifetime starts.
    /// </summary>
    public static InvokeRequest? PendingInvoke { get; set; }

    /// <summary>
    /// Set when native initialisation failed. The window shows the message
    /// instead of a broken UI — a P/Invoke failure here means
    /// libotterzip_ffi.so is missing or ABI-mismatched, and every button
    /// would throw.
    /// </summary>
    public static string? StartupError { get; private set; }

    public override void Initialize() => AvaloniaXamlLoader.Load(this);

    public override void OnFrameworkInitializationCompleted()
    {
        ApplyTheme();

        try
        {
            OtterzipLibrary.Initialize();
            OtterzipTelemetry.Initialize();
        }
        catch (Exception ex) when (ex is OtterzipException or DllNotFoundException or EntryPointNotFoundException)
        {
            StartupError = ex.Message;
        }

        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            // Double-clicking one archive should open ONLY its contents view —
            // not the contents view stacked on top of the drop window, which is
            // what happened when MainWindow was always the main window and then
            // spawned a second one. When the launch is exactly "open this one
            // archive", the contents view IS the main window; everything else
            // (a plain launch, a compress/extract verb, a multi-file open) is
            // the drop window as before.
            desktop.MainWindow = MainWindow.IsSingleArchiveOpen(PendingInvoke) && StartupError is null
                ? new ArchiveWindow(PendingInvoke!.Paths[0])
                : new MainWindow(PendingInvoke);
            // The window owns the shutdown decision: a headless verb closes
            // itself when its job settles, a plain launch waits for the user.
            desktop.ShutdownMode = ShutdownMode.OnMainWindowClose;
            desktop.Exit += (_, _) =>
            {
                OtterzipLibrary.Shutdown();
            };
        }

        base.OnFrameworkInitializationCompleted();
    }

    /// <summary>
    /// Apply <c>Settings_Theme</c>. "system" leaves Avalonia's Default
    /// variant in place, which follows the desktop's colour scheme (the
    /// org.freedesktop.appearance portal on a modern desktop, the GTK theme
    /// otherwise) and keeps following it if the user flips it while OtterZip
    /// is open.
    /// </summary>
    public static void ApplyTheme()
    {
        string theme = SettingsService.Get<string>("Settings_Theme", "system");
        Application? app = Current;
        if (app is null)
        {
            return;
        }
        app.RequestedThemeVariant = theme switch
        {
            "light" => ThemeVariant.Light,
            "dark" => ThemeVariant.Dark,
            _ => ThemeVariant.Default,
        };
    }
}
