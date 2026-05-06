using System;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Windows.ApplicationModel.Resources;
using SpanZIP.App.Services;

namespace SpanZIP.App.Dialogs;

/// <summary>
/// Phase 6+ rev 3 — standalone Settings Window with NavigationView (Top, 6 tabs).
/// Replaces the in-process ContentDialog from Sprint A and the rev 2 4-tab
/// shell so the surface matches Keka's settings layout more closely.
///
/// Tabs: General / Compression / Extraction / Password / Integration / Info.
/// Live-save: every section's controls write to <see cref="SettingsService"/>
/// directly. Closing the window is the commit gesture (no OK button).
/// </summary>
public sealed partial class SettingsWindow : Window
{
    private readonly GeneralSettingsSection _general = new();
    private readonly CompressionSettingsSection _compression = new();
    private readonly ExtractionSettingsSection _extraction = new();
    private readonly PasswordSettingsSection _password = new();
    private readonly IntegrationSettingsSection _integration = new();
    private readonly InfoSettingsSection _info = new();

    public SettingsWindow()
    {
        InitializeComponent();
        Title = new ResourceLoader().GetString("Settings_DialogTitle/Title");

        if (Content is FrameworkElement root)
        {
            ThemeService.Apply(root, ThemeService.Load());
        }

        SystemBackdrop = new Microsoft.UI.Xaml.Media.MicaBackdrop
        {
            Kind = Microsoft.UI.Composition.SystemBackdrops.MicaKind.BaseAlt,
        };
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);

        // rev 5: NavigationView moved from Top to Left sidebar — needs
        // ~180px for the pane plus the same content area, so widen to 780.
        TrySizeWindow(width: 780, height: 640);

        // Wire HWND resolvers for sections that host a FolderPicker.
        _compression.HostWindowResolver = () => this;
        _extraction.HostWindowResolver = () => this;

        // Default to General on open.
        Nav.SelectedItem = Nav.MenuItems[0];
    }

    private void OnNavSelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItem is NavigationViewItem item && item.Tag is string tag)
        {
            ContentHost.Children.Clear();
            UIElement section = tag switch
            {
                "compression" => _compression,
                "extraction" => _extraction,
                "password" => _password,
                "integration" => _integration,
                "info" => _info,
                _ => _general,
            };
            ContentHost.Children.Add(section);
        }
    }

    private void TrySizeWindow(int width, int height)
    {
        try
        {
            var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
            var windowId = Microsoft.UI.Win32Interop.GetWindowIdFromWindow(hwnd);
            var appWindow = Microsoft.UI.Windowing.AppWindow.GetFromWindowId(windowId);
            uint dpi = GetDpiForWindow(hwnd);
            double scale = dpi / 96.0;
            int physW = (int)(width * scale);
            int physH = (int)(height * scale);
            appWindow.Resize(new Windows.Graphics.SizeInt32(physW, physH));

            // Settings is a tabbed dialog with a fixed-grid layout — same
            // rationale as MainWindow: lock the size, drop the maximize
            // affordance, keep minimize for "get it out of my way".
            if (appWindow.Presenter is Microsoft.UI.Windowing.OverlappedPresenter presenter)
            {
                presenter.IsResizable = false;
                presenter.IsMaximizable = false;
            }
        }
        catch (Exception)
        {
            // Best-effort sizing — fall back to OS default.
        }
    }

    [System.Runtime.InteropServices.DllImport("user32.dll", ExactSpelling = true)]
    private static extern uint GetDpiForWindow(IntPtr hwnd);
}
