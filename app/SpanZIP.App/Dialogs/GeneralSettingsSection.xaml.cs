using System;
using System.Diagnostics;
using System.Globalization;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using SpanZIP.App.Services;
using Windows.Globalization;

namespace SpanZIP.App.Dialogs;

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

    private void LoadFromService()
    {
        // Language
        string lang = SettingsService.Get<string>("Settings_Language", "");
        LanguageCombo.SelectedIndex = lang switch
        {
            "ko" => 1,
            "en" => 2,
            _ => 0,
        };

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
    }

    private void OnLanguageChanged(object sender, SelectionChangedEventArgs e)
    {
        if (LanguageCombo.SelectedItem is ComboBoxItem item && item.Tag is string tag)
        {
            SettingsService.Set("Settings_Language", tag);
            // Package-identity gated — see App.OnLaunched for context.
            try { ApplicationLanguages.PrimaryLanguageOverride = tag; }
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
