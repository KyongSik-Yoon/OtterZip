using System.Globalization;
using Microsoft.Windows.ApplicationModel.Resources;

namespace SpanZIP.App.Services;

/// <summary>
/// Centralised wrapper around <see cref="ResourceLoader"/>. All user-facing
/// text in code MUST go through here — never literals — so we can keep
/// CONVENTIONS.md §3.8 ("no hardcoded strings") in one place.
///
/// XAML continues to use <c>x:Uid</c> for static text; this helper covers
/// the runtime-formatted cases (status bar, dialog titles, error toasts).
/// </summary>
public static class Strings
{
    private static readonly ResourceLoader s_loader = new();

    /// <summary>Lookup a localized string by its <c>name</c> in <c>Resources.resw</c>.</summary>
    public static string Get(string key) => s_loader.GetString(key);

    /// <summary>
    /// Lookup + <see cref="string.Format(IFormatProvider, string, object[])"/>
    /// using <see cref="CultureInfo.CurrentCulture"/> so number / date placeholders
    /// follow the user's locale.
    /// </summary>
    public static string Format(string key, params object[] args)
    {
        var template = s_loader.GetString(key);
        return string.Format(CultureInfo.CurrentCulture, template, args);
    }
}
