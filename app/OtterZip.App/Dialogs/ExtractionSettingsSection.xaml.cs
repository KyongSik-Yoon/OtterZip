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

        AlwaysExtractToSubfolderCheck.IsChecked = SettingsService.Get<bool>("Settings_AlwaysExtractToSubfolder", true);
        PreserveZoneIdCheck.IsChecked           = SettingsService.Get<bool>("Settings_PreserveZoneIdentifier", true);
        PlaySoundOnExtractCheck.IsChecked       = SettingsService.Get<bool>("Settings_PlaySoundOnExtract", true);
        RevealAfterExtractCheck.IsChecked       = SettingsService.Get<bool>("Settings_RevealAfterExtract", true);
        DeleteArchiveAfterExtractCheck.IsChecked = SettingsService.Get<bool>("Settings_DeleteArchiveAfterExtract", false);
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
        if (sender is CheckBox cb && cb.Tag is string key)
        {
            SettingsService.Set(key, cb.IsChecked.GetValueOrDefault());
        }
    }
}
