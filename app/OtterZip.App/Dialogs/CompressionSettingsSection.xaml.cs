using System;
using System.Globalization;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using OtterZip.App.Services;
using Windows.Storage.Pickers;

namespace OtterZip.App.Dialogs;

/// <summary>
/// Settings → Compression tab (Phase 6+ rev 3). 9 items.
/// </summary>
public sealed partial class CompressionSettingsSection : UserControl
{
    public CompressionSettingsSection()
    {
        InitializeComponent();
        Loaded += (_, _) => LoadFromService();
    }

    /// <summary>
    /// SettingsWindow injects a callback so the FolderPicker can target the
    /// settings window directly.
    /// </summary>
    public Func<Window>? HostWindowResolver { get; set; }

    private void LoadFromService()
    {
        string fmt = SettingsService.Get<string>("Settings_DefaultFormat", "ZIP");
        DefaultFormatCombo.SelectedIndex = SelectFormatIndex(fmt);

        DefaultMethodCombo.SelectedIndex = Math.Clamp(
            SettingsService.Get<int>("Settings_DefaultMethodIndex", 2), 0, 3);

        string saveLoc = SettingsService.Get<string>("Settings_SaveLocation", "same");
        SaveLocationRadios.SelectedIndex =
            string.Equals(saveLoc, "custom", StringComparison.Ordinal) ? 1 : 0;
        SaveLocationPathBox.Text = SettingsService.Get<string>("Settings_SaveLocationPath", "");
        SaveLocationCustomRow.Visibility =
            string.Equals(saveLoc, "custom", StringComparison.Ordinal)
                ? Visibility.Visible : Visibility.Collapsed;

        UseParentFolderNameCheck.IsChecked       = SettingsService.Get<bool>("Settings_UseParentFolderName", true);
        FilenameTemplateBox.Text                 = SettingsService.Get<string>("Settings_FilenameTemplate", "");
        ExcludeSystemMetadataCheck.IsChecked     = SettingsService.Get<bool>("Settings_ExcludeSystemMetadata", true);
        VerifyAfterCompressCheck.IsChecked       = SettingsService.Get<bool>("Settings_VerifyAfterCompress", false);
        AlwaysPromptPasswordCheck.IsChecked      = SettingsService.Get<bool>("Settings_AlwaysPromptPassword", false);
        PlaySoundOnCompressCheck.IsChecked       = SettingsService.Get<bool>("Settings_PlaySoundOnCompress", true);
        RevealAfterCompressCheck.IsChecked       = SettingsService.Get<bool>("Settings_RevealAfterCompress", true);
        DeleteSourceAfterCompressCheck.IsChecked = SettingsService.Get<bool>("Settings_DeleteSourceAfterCompress", false);
    }

    private static int SelectFormatIndex(string format)
    {
        if (string.Equals(format, "7z", StringComparison.Ordinal)) return 1;
        if (string.Equals(format, "tar", StringComparison.Ordinal)) return 2;
        if (string.Equals(format, "tar.gz", StringComparison.Ordinal)) return 3;
        if (string.Equals(format, "tar.bz2", StringComparison.Ordinal)) return 4;
        if (string.Equals(format, "tar.xz", StringComparison.Ordinal)) return 5;
        return 0;
    }

    private void OnDefaultFormatChanged(object sender, SelectionChangedEventArgs e)
    {
        if (DefaultFormatCombo.SelectedItem is ComboBoxItem item && item.Tag is string tag)
        {
            SettingsService.Set("Settings_DefaultFormat", tag);
        }
    }

    private void OnDefaultMethodChanged(object sender, SelectionChangedEventArgs e)
    {
        if (DefaultMethodCombo.SelectedItem is ComboBoxItem item
            && item.Tag is string tag
            && int.TryParse(tag, NumberStyles.Integer, CultureInfo.InvariantCulture, out int v))
        {
            SettingsService.Set("Settings_DefaultMethodIndex", v);
        }
    }

    private void OnSaveLocationChanged(object sender, SelectionChangedEventArgs e)
    {
        if (SaveLocationRadios.SelectedItem is RadioButton rb && rb.Tag is string tag)
        {
            SettingsService.Set("Settings_SaveLocation", tag);
            SaveLocationCustomRow.Visibility =
                string.Equals(tag, "custom", StringComparison.Ordinal)
                    ? Visibility.Visible : Visibility.Collapsed;
        }
    }

    private async void OnBrowseSaveLocation(object sender, RoutedEventArgs e)
    {
        if (HostWindowResolver is null) return;
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(HostWindowResolver());
        var picker = new FolderPicker();
        WinRT.Interop.InitializeWithWindow.Initialize(picker, hwnd);
        picker.FileTypeFilter.Add("*");
        var folder = await picker.PickSingleFolderAsync();
        if (folder is null) return;
        SaveLocationPathBox.Text = folder.Path;
        SettingsService.Set("Settings_SaveLocationPath", folder.Path);
    }

    private void OnToggle(object sender, RoutedEventArgs e)
    {
        if (sender is CheckBox cb && cb.Tag is string key)
        {
            SettingsService.Set(key, cb.IsChecked.GetValueOrDefault());
        }
    }

    private void OnFilenameTemplateChanged(object sender, TextChangedEventArgs e)
    {
        // Live-save. Empty string is the "use default rule" sentinel
        // (Use_ParentFolderName / first source name) — MainWindow.PlanCompress
        // checks for empty before applying token replacement.
        SettingsService.Set("Settings_FilenameTemplate", FilenameTemplateBox.Text ?? string.Empty);
    }
}
