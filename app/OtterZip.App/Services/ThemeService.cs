using System;
using Microsoft.UI.Xaml;

namespace OtterZip.App.Services;

/// <summary>
/// Theme preference layer — backed by <see cref="SettingsService"/> with key
/// <c>"Theme"</c>. The Apply method remains the place that pushes the choice
/// onto a window's root element; storage is now delegated.
///
/// Phase 6+ refactor: was a self-contained LocalSettings wrapper in Sprint A.
/// </summary>
public static class ThemeService
{
    private const string SettingsKey = "Theme";

    /// <summary>Read the persisted preference. Defaults to System on first run.</summary>
    public static AppTheme Load()
    {
        int raw = SettingsService.Get<int>(SettingsKey, (int)AppTheme.System);
        return Enum.IsDefined(typeof(AppTheme), raw) ? (AppTheme)raw : AppTheme.System;
    }

    /// <summary>Persist the user's pick.</summary>
    public static void Save(AppTheme theme)
    {
        SettingsService.Set(SettingsKey, (int)theme);
    }

    /// <summary>
    /// Apply the requested theme to a window's root content. Pass the
    /// `Window.Content` (cast to <see cref="FrameworkElement"/>) — WinUI 3
    /// doesn't expose RequestedTheme on Window directly, so we set it on
    /// the root element instead.
    /// </summary>
    public static void Apply(FrameworkElement root, AppTheme theme)
    {
        ArgumentNullException.ThrowIfNull(root);
        root.RequestedTheme = theme switch
        {
            AppTheme.Light => ElementTheme.Light,
            AppTheme.Dark => ElementTheme.Dark,
            _ => ElementTheme.Default,
        };
    }
}
