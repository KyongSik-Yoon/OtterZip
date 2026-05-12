using System;
using System.Diagnostics;
using System.Globalization;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using OtterZip.App.Services;
using Windows.Globalization;

namespace OtterZip.App.Dialogs;

/// <summary>
/// Settings → General tab (Phase 6+ rev 3).
/// Live-saves every change to <see cref="SettingsService"/>.
/// </summary>
public sealed partial class GeneralSettingsSection : UserControl
{
    public GeneralSettingsSection()
    {
        InitializeComponent();
        Loaded += (_, _) => LoadFromService();
    }

    /// <summary>
    /// Map persisted short language tag to the dropdown's index. Order
    /// must mirror the ComboBoxItem order in the XAML; both grew from
    /// 3 to 11 entries when we added the 8 extra .resw locales.
    /// </summary>
    private static int LanguageTagToIndex(string tag) => tag switch
    {
        "ko" => 1,
        "en" => 2,
        "zh" => 3,
        "ja" => 4,
        "de" => 5,
        "fr" => 6,
        "es" => 7,
        "pt" => 8,
        "ru" => 9,
        "it" => 10,
        _    => 0,   // "" or unknown -> System
    };

    /// <summary>
    /// Map the user's short selector tag to the full IETF locale tag
    /// that <c>ApplicationLanguages.PrimaryLanguageOverride</c> wants.
    /// We ship one .resw folder per IETF tag (en-US / ko-KR / zh-CN /
    /// ja-JP / de-DE / fr-FR / es-ES / pt-BR / ru-RU / it-IT). Without
    /// this mapping a bare "zh" override would let Windows pick an
    /// arbitrary Chinese variant — pinning to "zh-CN" matches what we
    /// actually translated. Span's LocalizationService follows the
    /// same pattern (see D:\11.AI\Span\src\Span\Span\Services\
    /// LocalizationService.cs ApplyPrimaryLanguageOverride).
    /// </summary>
    private static string ShortTagToIetf(string tag) => tag switch
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
        _    => "",     // empty -> follow OS
    };

    private void LoadFromService()
    {
        // Language - persisted as a short tag (e.g. "ko"), mapped to the
        // dropdown index by LanguageTagToIndex above.
        string lang = SettingsService.Get<string>("Settings_Language", "");
        LanguageCombo.SelectedIndex = LanguageTagToIndex(lang);

        ThemeRadioButtons.SelectedIndex = (int)ThemeService.Load();

        // Default action
        string action = SettingsService.Get<string>("Settings_DefaultAction", "auto");
        DefaultActionCombo.SelectedIndex = action switch
        {
            "compress" => 1,
            "extract" => 2,
            _ => 0,
        };

        // Toggles — defaults match settings-catalog §3.1 rev 3.
        // QuitWhenLastClosed was retired in rev 6 (no system tray, no
        // sensible OFF semantics — see catalog §3.4 OUT).
        ConfirmExitCheck.IsChecked = SettingsService.Get<bool>("Settings_ConfirmExitWhileBusy", true);
        ShowToastCheck.IsChecked   = SettingsService.Get<bool>("Settings_ShowToast", true);

        // Concurrent jobs — JobQueue reads this at MainWindow ctor.
        int concurrency = Math.Clamp(
            SettingsService.Get<int>("Settings_ConcurrentJobs", 2), 1, 4);
        ConcurrentJobsCombo.SelectedIndex = concurrency - 1;
    }

    private void OnLanguageChanged(object sender, SelectionChangedEventArgs e)
    {
        if (LanguageCombo.SelectedItem is ComboBoxItem item && item.Tag is string tag)
        {
            SettingsService.Set("Settings_Language", tag);
            // Map "ko" -> "ko-KR" etc. so MRT picks the right .resw folder.
            // Package-identity gated — see App.OnLaunched for the unpackaged
            // fallback rationale.
            string ietf = ShortTagToIetf(tag);
            try { ApplicationLanguages.PrimaryLanguageOverride = ietf; }
            catch (InvalidOperationException) { }
            catch (System.Runtime.InteropServices.COMException) { }
        }
    }

    private void OnThemeSelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (ThemeRadioButtons.SelectedItem is RadioButton rb
            && rb.Tag is string tag
            && int.TryParse(tag, NumberStyles.Integer, CultureInfo.InvariantCulture, out int v))
        {
            var theme = (AppTheme)v;
            ThemeService.Save(theme);
            if (XamlRoot?.Content is FrameworkElement root)
            {
                ThemeService.Apply(root, theme);
            }
            if (App.HostWindow?.Content is FrameworkElement hostRoot)
            {
                ThemeService.Apply(hostRoot, theme);
            }
        }
    }

    private void OnDefaultActionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (DefaultActionCombo.SelectedItem is ComboBoxItem item && item.Tag is string tag)
        {
            SettingsService.Set("Settings_DefaultAction", tag);
        }
    }

    private void OnConcurrentJobsChanged(object sender, SelectionChangedEventArgs e)
    {
        if (ConcurrentJobsCombo.SelectedItem is ComboBoxItem item
            && item.Tag is string tag
            && int.TryParse(tag, NumberStyles.Integer, CultureInfo.InvariantCulture, out int v))
        {
            SettingsService.Set("Settings_ConcurrentJobs", Math.Clamp(v, 1, 4));
        }
    }

    private void OnToggle(object sender, RoutedEventArgs e)
    {
        if (sender is CheckBox cb && cb.Tag is string key)
        {
            SettingsService.Set(key, cb.IsChecked.GetValueOrDefault());
        }
    }

    private void OnOpenDefaultApps(object sender, RoutedEventArgs e)
    {
        // Windows 10/11 Settings → Default Apps deep link. Process.Start
        // with UseShellExecute lets the OS resolve the URI handler.
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = "ms-settings:defaultapps",
                UseShellExecute = true,
            };
            Process.Start(psi);
        }
        catch (Exception)
        {
            // Defensive — if the URI handler is missing (e.g. server SKU)
            // we silently no-op rather than crashing the settings window.
        }
    }
}
