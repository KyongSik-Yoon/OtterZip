using System;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using OtterZip.App.Services;
using Windows.Storage.Pickers;

namespace OtterZip.App.Dialogs;

/// <summary>
/// Settings → Extraction tab (Phase 6+ rev 3). 6 items.
/// </summary>
public sealed partial class ExtractionSettingsSection : UserControl
{
    public ExtractionSettingsSection()
    {
        InitializeComponent();
        Loaded += (_, _) => LoadFromService();
    }

    public Func<Window>? HostWindowResolver { get; set; }

    private void LoadFromService()
    {
        string loc = SettingsService.Get<string>("Settings_ExtractLocation", "same");
        ExtractLocationRadios.SelectedIndex =
            string.Equals(loc, "custom", StringComparison.Ordinal) ? 1 : 0;
        ExtractLocationPathBox.Text = SettingsService.Get<string>("Settings_ExtractLocationPath", "");
        ExtractLocationCustomRow.Visibility =
            string.Equals(loc, "custom", StringComparison.Ordinal)
                ? Visibility.Visible : Visibility.Collapsed;

        // Default OFF — Keka-style immediate extract. Hold Ctrl/Alt at
        // drop time for one-shot panel access without flipping this.
        AskBeforeExtractCheck.IsOn         = SettingsService.Get<bool>("Settings_AskBeforeExtract", false);
        AlwaysExtractToSubfolderCheck.IsOn = SettingsService.Get<bool>("Settings_AlwaysExtractToSubfolder", true);

        // OverwritePolicy enum int → radio index. Default 4 (Rename/keep-both).
        // Radio order: 0=Rename(4), 1=Always(1), 2=Skip/IfNewer(2), 3=Reject/Never(0).
        int overwrite = SettingsService.Get<int>("Settings_OverwritePolicy", 4);
        OverwritePolicyRadios.SelectedIndex = overwrite switch { 1 => 1, 2 => 2, 0 => 3, _ => 0 };

        // Double-click action: 0=dialog (default), 1=extract here, 2=smart.
        // Tag order matches the value, so index == value.
        int doubleClick = SettingsService.Get<int>("Settings_DoubleClickAction", 0);
        DoubleClickActionRadios.SelectedIndex = (doubleClick is >= 0 and <= 2) ? doubleClick : 0;
        PreserveZoneIdCheck.IsOn           = SettingsService.Get<bool>("Settings_PreserveZoneIdentifier", true);
        PlaySoundOnExtractCheck.IsOn       = SettingsService.Get<bool>("Settings_PlaySoundOnExtract", true);
        RevealAfterExtractCheck.IsOn       = SettingsService.Get<bool>("Settings_RevealAfterExtract", true);
        DeleteArchiveAfterExtractCheck.IsOn = SettingsService.Get<bool>("Settings_DeleteArchiveAfterExtract", false);
    }

    private void OnExtractLocationChanged(object sender, SelectionChangedEventArgs e)
    {
        if (ExtractLocationRadios.SelectedItem is RadioButton rb && rb.Tag is string tag)
        {
            SettingsService.Set("Settings_ExtractLocation", tag);
            ExtractLocationCustomRow.Visibility =
                string.Equals(tag, "custom", StringComparison.Ordinal)
                    ? Visibility.Visible : Visibility.Collapsed;
        }
    }

    private void OnOverwritePolicyChanged(object sender, SelectionChangedEventArgs e)
    {
        if (OverwritePolicyRadios.SelectedItem is RadioButton rb
            && rb.Tag is string tag
            && int.TryParse(tag, System.Globalization.CultureInfo.InvariantCulture, out int policy))
        {
            SettingsService.Set("Settings_OverwritePolicy", policy);
        }
    }

    private void OnDoubleClickActionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (DoubleClickActionRadios.SelectedItem is RadioButton rb
            && rb.Tag is string tag
            && int.TryParse(tag, System.Globalization.CultureInfo.InvariantCulture, out int action))
        {
            SettingsService.Set("Settings_DoubleClickAction", action);
        }
    }

    private async void OnBrowseExtractLocation(object sender, RoutedEventArgs e)
    {
        if (HostWindowResolver is null) return;
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(HostWindowResolver());
        var picker = new FolderPicker();
        WinRT.Interop.InitializeWithWindow.Initialize(picker, hwnd);
        picker.FileTypeFilter.Add("*");
        var folder = await picker.PickSingleFolderAsync();
        if (folder is null) return;
        ExtractLocationPathBox.Text = folder.Path;
        SettingsService.Set("Settings_ExtractLocationPath", folder.Path);
    }

    private void OnToggle(object sender, RoutedEventArgs e)
    {
        if (sender is ToggleSwitch ts && ts.Tag is string key)
        {
            SettingsService.Set(key, ts.IsOn);
        }
    }
}
