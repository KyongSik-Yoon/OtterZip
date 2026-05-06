using System;
using System.IO;
using System.Threading.Tasks;
using Microsoft.UI.Xaml.Controls;
using SpanZIP.App.Services;

namespace SpanZIP.App.Dialogs;

// `ExtractDialogResult` lives in its own file (analyzer MA0048).

/// <summary>
/// Sprint A — S5 destination chooser. Replaces the silent
/// "always extract to sibling folder" behaviour with an explicit
/// 3-button picker (Primary = custom path, Secondary = sibling,
/// Close = cancel).
/// </summary>
public sealed partial class ExtractDialog : ContentDialog
{
    private readonly string _archivePath;
    private readonly string _siblingPath;

    public ExtractDialog(string archivePath)
    {
        ArgumentException.ThrowIfNullOrEmpty(archivePath);
        InitializeComponent();
        _archivePath = archivePath;

        // Phase 6+ rev 4: respect Settings_ExtractLocation (same / custom)
        // and Settings_AlwaysExtractToSubfolder. The sibling default still
        // exists as a fallback so the dialog never starts blank.
        string baseDir = Path.GetDirectoryName(archivePath) ?? Directory.GetCurrentDirectory();
        string stem = Path.GetFileNameWithoutExtension(archivePath);
        if (string.IsNullOrWhiteSpace(stem))
        {
            stem = "extracted";
        }

        string extractLoc = SettingsService.Get<string>("Settings_ExtractLocation", "same");
        if (string.Equals(extractLoc, "custom", StringComparison.Ordinal))
        {
            string customDir = SettingsService.Get<string>("Settings_ExtractLocationPath", "");
            if (!string.IsNullOrWhiteSpace(customDir) && Directory.Exists(customDir))
            {
                baseDir = customDir;
            }
        }

        // AlwaysExtractToSubfolder ON: dest = baseDir/<stem>/.
        // OFF: dest = baseDir/ (entries land directly under the chosen folder).
        bool useSubfolder = SettingsService.Get<bool>("Settings_AlwaysExtractToSubfolder", true);
        _siblingPath = useSubfolder ? Path.Combine(baseDir, stem) : baseDir;
        DestinationField.Text = _siblingPath;

        // Compose localised title `Extract {fileName}`.
        Title = Strings.Format("ExtractDialog_TitleFormat", Path.GetFileName(archivePath));
        PrimaryButtonText = Strings.Get("ExtractDialog_PrimaryButton/Text");
        SecondaryButtonText = Strings.Get("ExtractDialog_SecondaryButton/Text");
        CloseButtonText = Strings.Get("ExtractDialog_CancelButton/Text");
    }

    /// <summary>The path the user picked. Valid only when the result is
    /// <see cref="ExtractDialogResult.UseCustomPath"/> or
    /// <see cref="ExtractDialogResult.ExtractHere"/>.</summary>
    public string ChosenPath { get; private set; } = string.Empty;

    /// <summary>Show the dialog and return the user's choice.</summary>
    public new async Task<ExtractDialogResult> ShowAsync()
    {
        var result = await base.ShowAsync();
        switch (result)
        {
            case ContentDialogResult.Primary:
                ChosenPath = string.IsNullOrWhiteSpace(DestinationField.Text)
                    ? _siblingPath
                    : DestinationField.Text;
                return ExtractDialogResult.UseCustomPath;
            case ContentDialogResult.Secondary:
                ChosenPath = _siblingPath;
                return ExtractDialogResult.ExtractHere;
            default:
                return ExtractDialogResult.Cancel;
        }
    }

    private async void OnBrowseClick(object sender, Microsoft.UI.Xaml.RoutedEventArgs e) // CA1822
    {
        var picker = new Windows.Storage.Pickers.FolderPicker();
        // ContentDialog has no Window handle of its own; we resolve the
        // dialog's XamlRoot host via App.MainWindow if available.
        if (App.HostWindow is { } host)
        {
            WinRT.Interop.InitializeWithWindow.Initialize(
                picker,
                WinRT.Interop.WindowNative.GetWindowHandle(host));
        }
        picker.FileTypeFilter.Add("*");
        picker.SuggestedStartLocation = Windows.Storage.Pickers.PickerLocationId.Desktop;
        var folder = await picker.PickSingleFolderAsync();
        if (folder is not null)
        {
            DestinationField.Text = folder.Path;
        }
    }
}
