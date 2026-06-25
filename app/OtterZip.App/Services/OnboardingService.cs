using System;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using OtterZip.App.Dialogs;

namespace OtterZip.App.Services;

/// <summary>
/// Single entry point for the first-run onboarding window. App.OnLaunched
/// calls <see cref="ShowIfFirstRun"/> on a normal (non-headless) launch; if
/// onboarding was already completed/skipped the call is a no-op. Settings →
/// Info → "시작 안내 다시 보기" reuses <see cref="ShowAlways"/> on demand.
///
/// Mirrors the DragShelf (Maru) OnboardingService structure, but the window
/// itself is native WinUI (no WebView2 — OtterZip deliberately avoids it for
/// Native AOT).
/// </summary>
internal static class OnboardingService
{
    private const string CompleteKey = "Settings_OnboardingComplete";

    private static OnboardingWindow? _activeWindow;

    public static void ShowIfFirstRun(DispatcherQueue dispatcher)
    {
        if (SettingsService.Get<bool>(CompleteKey, false))
        {
            return;
        }
        // Mark complete the moment we decide to show it — so onboarding runs
        // exactly once even if the user closes the window with [X] (or it
        // dies) before reaching Finish/Skip. Re-opening from Settings uses
        // ShowAlways directly and is unaffected (the flag is already set).
        MarkComplete();
        ShowAlways(dispatcher);
    }

    public static void ShowAlways(DispatcherQueue dispatcher)
    {
        ArgumentNullException.ThrowIfNull(dispatcher);

        // Guard against double-show: bring the existing window forward
        // instead of stacking another instance.
        if (_activeWindow is not null)
        {
            try { _activeWindow.Activate(); }
            catch (Exception) { /* window may be mid-close */ }
            return;
        }

        dispatcher.TryEnqueue(() =>
        {
            try
            {
                _activeWindow = new OnboardingWindow();
                _activeWindow.Closed += OnWindowClosed;
                _activeWindow.Activate();
            }
            catch (Exception ex)
            {
                System.Diagnostics.Debug.WriteLine($"[OnboardingService] open failed: {ex.Message}");
                // Mark complete so a broken onboarding never loops the user
                // out of the app on every launch.
                MarkComplete();
                _activeWindow = null;
            }
        });
    }

    /// <summary>Persist that onboarding is done — called by the window on
    /// finish/skip and as the failure fallback above.</summary>
    public static void MarkComplete() => SettingsService.Set(CompleteKey, true);

    private static void OnWindowClosed(object sender, WindowEventArgs args) => _activeWindow = null;
}
