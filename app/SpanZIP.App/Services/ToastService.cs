using System;
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;

namespace SpanZIP.App.Services;

/// <summary>
/// Thin wrapper around <see cref="AppNotificationManager"/> for SpanZIP's
/// completion notifications. Gated by the
/// <c>Settings_ShowToast</c> preference — when off, every call is a no-op.
///
/// WindowsAppSDK 1.2+ surface; works under both packaged and unpackaged
/// runs. The manager is lazily registered on first use to avoid paying
/// the COM activation cost during cold start when toasts are off.
/// </summary>
public static class ToastService
{
    private static bool s_registered;

    /// <summary>
    /// Show a "compress / extract complete" toast with a single line of
    /// detail text. Silent when:
    ///   * <c>Settings_ShowToast</c> is off, or
    ///   * the AppNotificationManager registration fails (unpackaged
    ///     environments without an AppUserModelId can throw COM errors).
    /// </summary>
    public static void ShowCompletion(string title, string body)
    {
        ArgumentException.ThrowIfNullOrEmpty(title);
        if (!SettingsService.Get<bool>("Settings_ShowToast", true))
        {
            return;
        }
        try
        {
            EnsureRegistered();
            var notification = new AppNotificationBuilder()
                .AddText(title)
                .AddText(body ?? string.Empty)
                .BuildNotification();
            AppNotificationManager.Default.Show(notification);
        }
        catch (Exception)
        {
            // Toast is courtesy — failing to register / show must never
            // surface to the user. Common failure: unpackaged run without
            // an installer-registered AppUserModelId.
        }
    }

    private static void EnsureRegistered()
    {
        if (s_registered) return;
        AppNotificationManager.Default.Register();
        s_registered = true;
    }
}
