using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Windows.UI;

namespace OtterZip.App.Services;

/// <summary>
/// Shared window-chrome helpers for the standalone modal windows
/// (ProgressDialog / CompressOptionsDialog / ExtractOptionsDialog).
/// </summary>
internal static class WindowChrome
{
    /// <summary>
    /// Colour the standard caption bar to match the dark/light body so the
    /// window reads as one surface instead of a dark panel under a white
    /// title bar. The AppWindowTitleBar API takes <see cref="Color"/> (not a
    /// brush), so these literals — mirroring Themes/Colors.xaml — are the one
    /// place a hardcoded colour is unavoidable. No-op on OS builds without
    /// title-bar customization support.
    /// </summary>
    public static void ApplyTitleBarTheme(AppWindow? appWindow)
    {
        if (appWindow is null || !AppWindowTitleBar.IsCustomizationSupported())
        {
            return;
        }
        bool dark = ResolveDark();
        Color bg = dark ? Color.FromArgb(0xFF, 0x13, 0x15, 0x19) : Color.FromArgb(0xFF, 0xFA, 0xFB, 0xFC);
        Color fg = dark ? Color.FromArgb(0xFF, 0xEC, 0xEE, 0xF0) : Color.FromArgb(0xFF, 0x1B, 0x1F, 0x24);
        Color dim = dark ? Color.FromArgb(0xFF, 0x71, 0x77, 0x83) : Color.FromArgb(0xFF, 0x8A, 0x90, 0x9C);
        Color hover = dark ? Color.FromArgb(0x18, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x14, 0x00, 0x00, 0x00);
        Color press = dark ? Color.FromArgb(0x28, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x24, 0x00, 0x00, 0x00);

        var tb = appWindow.TitleBar;
        tb.BackgroundColor = bg;
        tb.InactiveBackgroundColor = bg;
        tb.ForegroundColor = fg;
        tb.InactiveForegroundColor = dim;
        tb.ButtonBackgroundColor = bg;
        tb.ButtonInactiveBackgroundColor = bg;
        tb.ButtonForegroundColor = fg;
        tb.ButtonInactiveForegroundColor = dim;
        tb.ButtonHoverBackgroundColor = hover;
        tb.ButtonHoverForegroundColor = fg;
        tb.ButtonPressedBackgroundColor = press;
        tb.ButtonPressedForegroundColor = fg;
    }

    /// <summary>Effective dark/light — honours the app override, else the
    /// actual system theme.</summary>
    private static bool ResolveDark() => ThemeService.Load() switch
    {
        AppTheme.Dark => true,
        AppTheme.Light => false,
        _ => IsSystemDark(),
    };

    /// <summary>
    /// Whether Windows is currently in dark mode. Derived from the OS
    /// window-background colour — <see cref="Application.RequestedTheme"/>
    /// does NOT reliably track the live system theme in WinUI 3, which left
    /// the caption bar dark on a light system (body followed the theme, the
    /// title bar didn't). Reading UISettings keeps them in sync.
    /// </summary>
    private static bool IsSystemDark()
    {
        try
        {
            Color bg = new Windows.UI.ViewManagement.UISettings()
                .GetColorValue(Windows.UI.ViewManagement.UIColorType.Background);
            // Perceived luminance; the dark theme has a near-black background.
            return ((5 * bg.G) + (2 * bg.R) + bg.B) < (8 * 128);
        }
        catch (System.Exception)
        {
            return Application.Current.RequestedTheme == ApplicationTheme.Dark;
        }
    }
}
