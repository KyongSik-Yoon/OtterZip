using System;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using OtterZip.App.Services;

namespace OtterZip.App.Dialogs;

/// <summary>
/// Settings → Integration tab (Phase 6+ rev 3). 3 items, all gated on
/// Sprint S body availability — controls show the persisted state but
/// remain disabled until the COM body is wired.
/// </summary>
public sealed partial class IntegrationSettingsSection : UserControl
{
    public IntegrationSettingsSection()
    {
        InitializeComponent();
        Loaded += (_, _) =>
        {
            ShellMenuEnabledCheck.IsOn       = SettingsService.Get<bool>("Settings_ShellMenuEnabled", true);
            QuickProgressAutoCloseCheck.IsOn = SettingsService.Get<bool>("Settings_QuickProgressAutoClose", false);
            // 2026-05-15: default flipped "nested" → "flat" so the
            // Bandizip-style quick verbs surface at Explorer's top
            // level by default. Mirrors ShellSettings.cpp on the C++
            // side; both must stay in sync or the radio reads back
            // different than what the shell actually applies.
            string mode = SettingsService.Get<string>("Settings_ShellMenuMode", "flat");
            ShellMenuModeRadios.SelectedIndex =
                string.Equals(mode, "flat", StringComparison.Ordinal) ? 0 : 1;
        };
    }

    private void OnShellMenuModeChanged(object sender, SelectionChangedEventArgs e)
    {
        if (ShellMenuModeRadios.SelectedItem is RadioButton rb && rb.Tag is string tag)
        {
            SettingsService.Set("Settings_ShellMenuMode", tag);
        }
    }

    private void OnToggle(object sender, RoutedEventArgs e)
    {
        if (sender is ToggleSwitch ts && ts.Tag is string key)
        {
            SettingsService.Set(key, ts.IsOn);
        }
    }
}
