namespace SpanZIP.App.Services;

/// <summary>
/// User-selectable theme. Persisted by <see cref="ThemeService"/>.
/// `System` is the default — follow Windows light/dark setting.
/// </summary>
public enum AppTheme
{
    System = 0,
    Light = 1,
    Dark = 2,
}
