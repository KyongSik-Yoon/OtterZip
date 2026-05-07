using System;
using System.IO;
using System.Threading.Tasks;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using OtterZip.App.Services;
using OtterZip.Interop;

namespace OtterZip.App.Dialogs;

public sealed partial class InfoSettingsSection : UserControl
{
    public InfoSettingsSection()
    {
        InitializeComponent();
        Loaded += (_, _) =>
        {
            VersionText.Text = $"OtterZip {AssemblyInfoService.Version} "
                + $"(build {AssemblyInfoService.GitHash}, {AssemblyInfoService.BuildDate})";
            TelemetryEnabledCheck.IsChecked =
                SettingsService.Get<bool>("Settings_TelemetryEnabled", false);
        };
    }

    private void OnTelemetryToggle(object sender, RoutedEventArgs e)
    {
        bool optedIn = TelemetryEnabledCheck.IsChecked.GetValueOrDefault();
        SettingsService.Set("Settings_TelemetryEnabled", optedIn);
        // Apply immediately — Sentry init/dispose is idempotent so flipping
        // back-and-forth doesn't accumulate state.
        OtterzipTelemetry.SetUserOptIn(optedIn);
    }

    private async void OnOpenLicense(object sender, RoutedEventArgs e)
        => await ShowFileContentDialogAsync("LICENSE.txt").ConfigureAwait(true);

    private async void OnOpenThirdParty(object sender, RoutedEventArgs e)
        => await ShowFileContentDialogAsync("THIRD-PARTY-NOTICES.md").ConfigureAwait(true);

    private async Task ShowFileContentDialogAsync(string fileName)
    {
        if (XamlRoot is null) return;
        string path = Path.Combine(AppContext.BaseDirectory, fileName);
        string content;
        try
        {
            content = File.Exists(path)
                ? await File.ReadAllTextAsync(path).ConfigureAwait(true)
                : $"({fileName} not found in installation)";
        }
        catch (IOException ex)
        {
            content = ex.Message;
        }
        var dialog = new ContentDialog
        {
            Title = fileName,
            Content = new ScrollViewer
            {
                MaxHeight = 480,
                Content = new TextBlock
                {
                    Text = content,
                    TextWrapping = TextWrapping.Wrap,
                    FontFamily = (Microsoft.UI.Xaml.Media.FontFamily)Application.Current.Resources["MonoFontFamily"],
                    FontSize = 12,
                },
            },
            CloseButtonText = "Close",
            XamlRoot = XamlRoot,
        };
        await dialog.ShowAsync();
    }
}
